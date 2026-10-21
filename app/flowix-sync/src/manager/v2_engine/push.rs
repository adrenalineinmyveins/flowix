use super::*;

impl SyncManager {
    pub(super) async fn push_v2_inflight(
        &self,
        access_token: &str,
        due: &[crate::v2::V2InflightOperation],
        generation: u64,
    ) -> Result<(usize, usize, HashSet<i64>, Vec<V2RemoteApply>), SyncError> {
        let mut uploaded = 0;
        let mut deleted = 0;
        // P0-1: 记录本批次 push 在服务端拿到的 sync_seq，回传给 pull 阶段，用于过滤
        // 本端自己的回声（单端 push 后 pull 用旧 cursor 又拉回自己刚推上去的内容）。
        let mut self_sync_seqs: HashSet<i64> = HashSet::new();
        // thread 409 自动合并的产物（merged payload），交由桥接层落库刷新 UI。
        let mut merged_applies: Vec<V2RemoteApply> = Vec::new();
        for batch in due.chunks(100) {
            drop(self.require_auth_generation(generation)?);
            let operations = batch
                .iter()
                .map(|item| {
                    serde_json::from_str::<V2PushOperation>(&item.payload_json).map_err(|error| {
                        SyncError::InvalidState(format!("invalid frozen v2 operation: {error}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = match self.client.v2_push(access_token, &operations).await {
                Ok(result) => result,
                Err(error) => {
                    let _generation = self.require_auth_generation(generation)?;
                    for item in batch {
                        self.store.defer_v2_operation(
                            &item.operation_id,
                            Utc::now().timestamp_millis(),
                            &error.to_string(),
                        )?;
                    }
                    return Err(error);
                }
            };
            // 校验 generation 后立即释放 guard：循环内的 thread 冲突消解
            // 是 async，持有 std MutexGuard 跨 await 会让 future 退化为 !Send。
            drop(self.require_auth_generation(generation)?);
            let by_id: HashMap<&str, &crate::v2::V2OperationResult> = result
                .results
                .iter()
                .map(|item| (item.operation_id.as_str(), item))
                .collect();
            let operations_by_id: HashMap<&str, &V2PushOperation> = operations
                .iter()
                .map(|operation| {
                    let operation_id = match operation {
                        V2PushOperation::NotebookPut { operation_id, .. }
                        | V2PushOperation::NotebookDelete { operation_id, .. }
                        | V2PushOperation::NotePut { operation_id, .. }
                        | V2PushOperation::NoteDelete { operation_id, .. }
                        | V2PushOperation::NoteMove { operation_id, .. }
                        | V2PushOperation::ThreadPut { operation_id, .. }
                        | V2PushOperation::ThreadDelete { operation_id, .. } => operation_id,
                    };
                    (operation_id.as_str(), operation)
                })
                .collect();
            for item in batch {
                let response = by_id.get(item.operation_id.as_str()).ok_or_else(|| {
                    SyncError::InvalidState(format!(
                        "cloud omitted operation result {}",
                        item.operation_id
                    ))
                })?;
                if response.ok {
                    let data = response.data.as_ref().ok_or_else(|| {
                        SyncError::InvalidState(format!(
                            "cloud omitted successful operation data {}",
                            item.operation_id
                        ))
                    })?;
                    let operation = operations_by_id
                        .get(item.operation_id.as_str())
                        .ok_or_else(|| {
                            SyncError::InvalidState(format!(
                                "local push batch omitted operation {}",
                                item.operation_id
                            ))
                        })?;
                    self_sync_seqs.insert(data.sync_seq);
                    self.store.acknowledge_v2_operation(
                        &item.operation_id,
                        item.entity_type,
                        &item.entity_id,
                        item.generation,
                        operation,
                        data,
                    )?;
                    match item.operation_kind {
                        V2OperationKind::Put => uploaded += 1,
                        V2OperationKind::Delete => deleted += 1,
                    }
                } else {
                    if response.status == 409
                        && response
                            .error
                            .as_ref()
                            .is_some_and(|error| error.code == "REVISION_CONFLICT")
                    {
                        let current_revision = response
                            .error
                            .as_ref()
                            .and_then(|error| error.details.as_ref())
                            .and_then(|details| details.get("currentRevision"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string();
                        // thread 冲突走消息级合并自动消解（方案 A 核心）；
                        // note/notebook 仍走桌面冲突副本流程。
                        if item.entity_type == V2EntityType::Thread {
                            match self
                                .resolve_thread_push_conflict(
                                    access_token,
                                    item,
                                    &current_revision,
                                    generation,
                                )
                                .await
                            {
                                Ok(Some(merged)) => {
                                    uploaded += 1;
                                    merged_applies.push(merged);
                                    continue;
                                }
                                Ok(None) => {
                                    // 远端已删除：本地 delete 目的已达，视为成功。
                                    deleted += 1;
                                    continue;
                                }
                                Err(error) => {
                                    // 合并失败（网络等）：defer 走指数退避，下轮重试。
                                    self.store.defer_v2_operation(
                                        &item.operation_id,
                                        Utc::now().timestamp_millis(),
                                        &format!("thread merge deferred: {error}"),
                                    )?;
                                    continue;
                                }
                            }
                        }
                        // Never pull and acknowledge the newer cloud head while
                        // this frozen local edit still targets an older base.
                        // Doing so would cause the next scan to upload the local
                        // bytes as a fresh edit and silently replace the peer.
                        return Err(SyncError::RevisionConflict {
                            notebook_id: operations_by_id
                                .get(item.operation_id.as_str())
                                .and_then(|operation| match operation {
                                    V2PushOperation::NotePut { note, .. } => {
                                        Some(note.notebook_id.clone())
                                    }
                                    _ => None,
                                })
                                .unwrap_or_default(),
                            note_id: item.entity_id.clone(),
                            operation_id: item.operation_id.clone(),
                            operation_kind: item.operation_kind.as_str().to_string(),
                            current_revision,
                        });
                    }
                    let message = response
                        .error
                        .as_ref()
                        .map(|error| format!("{}: {}", error.code, error.message))
                        .unwrap_or_else(|| {
                            format!("operation failed with status {}", response.status)
                        });
                    self.store.defer_v2_operation(
                        &item.operation_id,
                        Utc::now().timestamp_millis(),
                        &message,
                    )?;
                }
            }
        }
        Ok((uploaded, deleted, self_sync_seqs, merged_applies))
    }

    /// thread push 409 消解（方案 A §6.5，两段式）：
    ///
    /// 冻结操作只含元数据（完整消息在 blob），引擎不在 409 当轮强行合并，
    /// 而是：拉远端 payload → 丢弃旧 inflight（**保留 dirty**）→ 以
    /// `V2RemoteApply::Thread` 返回远端内容。桥接层 upsert（消息按 id
    /// 幂等并集，`v2_merge_thread_payloads` 同款语义）后 re-dirty，
    /// 下轮 freeze 以远端头为 base 推合并后的全量——本地消息零丢失。
    ///
    /// - 远端已删 + 本地 delete → 目的已达，按远端头结算（无 apply）。
    /// - 远端已删 + 本地 put → 编辑胜过删除：重置本地基线走 null-base 复活。
    /// - 远端活跃 + 本地 delete → 保留 dirty，返回远端内容由桥提示用户。
    async fn resolve_thread_push_conflict(
        &self,
        access_token: &str,
        item: &crate::v2::V2InflightOperation,
        current_revision: &str,
        generation: u64,
    ) -> Result<Option<V2RemoteApply>, SyncError> {
        let thread_id = item.entity_id.as_str();
        // bootstrap.threads 不分页（A1 服务端全量输出），第一页必含全部线程。
        let bootstrap = self.client.v2_bootstrap_page(access_token, None).await?;
        let remote = bootstrap
            .threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .ok_or_else(|| {
                SyncError::InvalidState(format!(
                    "conflicting cloud thread {thread_id} disappeared"
                ))
            })?;
        drop(self.require_auth_generation(generation)?);
        if remote.deleted {
            return match item.operation_kind {
                V2OperationKind::Delete => {
                    self.store.settle_v2_thread_delete_as_remote_head(
                        item,
                        &remote.revision,
                        remote.sync_seq,
                    )?;
                    Ok(None)
                }
                V2OperationKind::Put => {
                    self.store.reset_v2_thread_for_recreate(item)?;
                    Ok(None)
                }
            };
        }
        if remote.revision != current_revision {
            return Err(SyncError::InvalidState(
                "cloud thread head moved during conflict resolution".into(),
            ));
        }
        let (payload, content) = match remote.content_hash.as_deref() {
            Some(hash) => {
                let bytes = self
                    .download_verified_v2_blob(access_token, thread_id, hash)
                    .await?;
                let payload: crate::v2::V2ThreadPayload =
                    serde_json::from_slice(&bytes).map_err(|error| {
                        SyncError::InvalidState(format!("parse remote thread payload: {error}"))
                    })?;
                (payload, bytes)
            }
            None => return Err(SyncError::InvalidState("cloud thread head has no content".into())),
        };
        self.store.defer_thread_conflict_to_resync(item)?;
        Ok(Some(V2RemoteApply::Thread {
            thread_id: thread_id.to_string(),
            agent_id: payload.agent_id.clone(),
            title: payload.title.clone(),
            content_hash: remote.content_hash.clone(),
            content: Some(content),
            revision: remote.revision.clone(),
            sync_seq: remote.sync_seq,
            deleted: false,
        }))
    }
}
