use super::*;

impl SyncManager {
    async fn bootstrap_v2_snapshot(
        &self,
        access_token: &str,
        enabled_notebooks: &[crate::v2::V2SyncedNotebook],
        cursor: i64,
        generation: u64,
        started_at: i64,
        uploaded: usize,
        deleted: usize,
        remote_preresolved: Vec<V2RemoteApply>,
    ) -> Result<V2AccountSyncReport, SyncError> {
        let bootstrap = self.client.v2_bootstrap_page(access_token, None).await?;
        let enabled_ids: HashSet<_> = enabled_notebooks
            .iter()
            .map(|notebook| notebook.notebook_id.as_str())
            .collect();
        let mut remote = self
            .remote_from_bootstrap(access_token, &enabled_ids, &bootstrap)
            .await?;
        let complete = bootstrap.next_page_token.is_none();
        let bootstrapped_notebooks = if complete {
            enabled_notebooks
                .iter()
                .map(|notebook| notebook.notebook_id.clone())
                .collect()
        } else {
            Vec::new()
        };
        // 409 自动消解产物（merged/远端 thread）排在 bootstrap 内容之前，
        // 桥接层先落冲突内容再落快照。
        remote.extend(remote_preresolved);

        Ok(V2AccountSyncReport {
            auth_generation: Some(generation),
            started_at,
            cursor: if complete { bootstrap.cursor } else { cursor },
            head_cursor: bootstrap.cursor,
            uploaded,
            deleted,
            remote,
            bootstrapped_notebooks,
            bootstrap_next_page_token: bootstrap.next_page_token,
            threads_bootstrapped: complete,
        })
    }

    pub(super) async fn sync_v2_account_once(
        &self,
        access_token: &str,
        notebooks: &[V2LocalNotebook],
        notes: &[V2LocalNote],
        threads: &[crate::v2::V2LocalThread],
        notebook_scope: Option<&str>,
        generation: u64,
    ) -> Result<V2AccountSyncReport, SyncError> {
        let started_at = Utc::now().timestamp_millis();
        let enabled_notebooks: Vec<_> = self
            .store
            .v2_notebooks(true)?
            .into_iter()
            .filter(|notebook| notebook_scope.is_none_or(|scope| notebook.notebook_id == scope))
            .collect();
        if let Some(scope) = notebook_scope {
            if enabled_notebooks.is_empty() {
                return Err(SyncError::InvalidState(format!(
                    "notebook {scope} is not enabled for cloud sync"
                )));
            }
        }
        let enabled_ids: HashSet<&str> = enabled_notebooks
            .iter()
            .map(|notebook| notebook.notebook_id.as_str())
            .collect();
        // 副本风暴根因修复：bootstrap_required（登录后/首次启用/CURSOR_EXPIRED）
        // 时本轮**纯拉取**——reconcile/freeze/push 全部跳过。logout 清空
        // v2_note_states 但本地缓存文件保留，若先 push，遗留文件会以
        // base=None 撞上云端已有实体的严格检查（409），且 conflict material
        // 无 base_content（states 空）导致盲存冲突副本，链式放大成风暴。
        // 先 bootstrap 建立 states 基线，下轮 pass 再按需推送。
        let bootstrap_required = enabled_notebooks
            .iter()
            .any(|notebook| notebook.bootstrap_required)
            || (notebook_scope.is_none() && self.store.v2_threads_bootstrap_required()?);
        let mut self_sync_seqs: HashSet<i64> = HashSet::new();
        let (uploaded, deleted, merged_applies) = if bootstrap_required {
            (0, 0, Vec::new())
        } else {
            self_sync_seqs = self
                .push_v2_pending_moves(access_token, notebook_scope, generation)
                .await?;
            {
                let _generation = self.require_auth_generation(generation)?;
                self.reconcile_v2_snapshot(&enabled_ids, notebooks, notes)?;
            }
            self.freeze_new_v2_operations(
                access_token,
                notebooks,
                notes,
                threads,
                notebook_scope,
                generation,
            )
            .await?;

            // thread 是全局实体：只有账户级同步推进 thread 操作（scope 模式
            // 下 thread inflight 不参与，避免 notebook cursor 消费 thread change）。
            let due = match notebook_scope {
                Some(scope) => self
                    .store
                    .v2_inflight_due_for_notebook(Utc::now().timestamp_millis(), scope)?,
                None => self.store.v2_inflight_due(Utc::now().timestamp_millis())?,
            };
            let (uploaded, deleted, pushed_seqs, merged_applies) = self
                .push_v2_inflight(access_token, &due, generation)
                .await?;
            self_sync_seqs.extend(pushed_seqs);
            (uploaded, deleted, merged_applies)
        };

        let cursor = match notebook_scope {
            Some(scope) => self.store.v2_notebook_cursor(scope)?,
            None => self.store.v2_cursor()?,
        };
        let report = if bootstrap_required {
            self.bootstrap_v2_snapshot(
                access_token,
                &enabled_notebooks,
                cursor,
                generation,
                started_at,
                uploaded,
                deleted,
                merged_applies,
            )
            .await?
        } else {
            match self
                .pull_v2_changes(
                    access_token,
                    cursor,
                    &enabled_ids,
                    notebook_scope,
                    &self_sync_seqs,
                )
                .await
            {
                Ok((mut remote, next_cursor, head_cursor)) => {
                    remote.extend(merged_applies);
                    V2AccountSyncReport {
                        auth_generation: Some(generation),
                        started_at,
                        cursor: next_cursor,
                        head_cursor,
                        uploaded,
                        deleted,
                        remote,
                        bootstrapped_notebooks: Vec::new(),
                        bootstrap_next_page_token: None,
                        threads_bootstrapped: false,
                    }
                }
                Err(SyncError::Api {
                    status: 410, code, ..
                }) if code == "CURSOR_EXPIRED" => {
                    self.bootstrap_v2_snapshot(
                        access_token,
                        &enabled_notebooks,
                        cursor,
                        generation,
                        started_at,
                        uploaded,
                        deleted,
                        merged_applies,
                    )
                    .await?
                }
                Err(error) => return Err(error),
            }
        };

        let _generation = self.require_auth_generation(generation)?;
        Ok(report)
    }
}

#[cfg(test)]
mod bootstrap_first_tests {
    use crate::{models::CloudUser, RefreshTokenStore, SyncManager, V2CloudAccount};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct NullTokenStore;
    impl RefreshTokenStore for NullTokenStore {
        fn load(&self) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn save(&self, _token: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// 单连接 mock：所有请求都返回同一 body（bootstrap 空快照）。
    async fn spawn_single_response_mock(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 4096];
                let _ = socket.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    /// 副本风暴根因回归（修复 A）：threads_bootstrap_required（logout→login 或
    /// 首次接入）时，本轮必须是**纯 bootstrap 拉取**——即便 dirty 队列里有
    /// 待推实体也不 freeze/push。历史上 push 先行：logout 清空 v2_note_states
    /// 后遗留本地文件以 base=None 撞云端已有实体的严格检查（409），conflict
    /// material 又因 states 为空拿不到 base_content，盲存冲突副本链式放大。
    /// mock 只接受一个请求并返回 bootstrap body：若引擎先 push，push 响应
    /// 解析会因缺少 results 字段失败（或第二个请求无人应答）。
    #[tokio::test]
    async fn bootstrap_required_pass_skips_push_entirely() {
        let body = r#"{"data":{"protocolEpoch":2,"cursor":3,"serverTime":1,"notebooks":[],"notes":[],"threads":[],"usage":{"usedBytes":0,"quotaBytes":1073741824,"entitlementExpiresAt":null},"nextPageToken":null}}"#;
        let base = spawn_single_response_mock(body).await;
        let temp = tempfile::tempdir().unwrap();
        let manager = SyncManager::new_with_session_dependencies(
            &base,
            temp.path().join("sync.db"),
            Arc::new(NullTokenStore),
            Arc::new(|_| {}),
        )
        .unwrap();
        // 已登录（本地 v2 账户 + 内存 session），但从未完成 threads bootstrap
        manager
            .store
            .save_v2_account(&V2CloudAccount {
                user: CloudUser {
                    id: "usr_test".into(),
                    email: "t@t.local".into(),
                    display_name: "T".into(),
                    system_role: "user".into(),
                },
                protocol_epoch: 2,
            })
            .unwrap();
        manager.auth.write().unwrap().session = Some(crate::models::RuntimeSession {
            access_token: "access-test".into(),
            access_token_expires_at: i64::MAX,
            refresh_token: "refresh-test".into(),
            refresh_token_expires_at: i64::MAX,
        });
        assert!(manager.store.v2_threads_bootstrap_required().unwrap());
        // dirty thread 在队（logout 遗留的待推实体等价物）
        manager
            .record_v2_thread_change(
                "th_pending",
                crate::LocalChangeKind::Put,
                "fingerprint-1",
            )
            .unwrap();

        let report = manager
            .sync_v2_account(Vec::new(), Vec::new(), Vec::new())
            .await
            .expect("bootstrap-only pass must succeed");
        assert!(report.threads_bootstrapped, "本轮为 bootstrap 完成");
        assert_eq!(report.uploaded, 0, "bootstrap 轮不得推送任何实体");
        assert!(!report.remote.iter().any(|apply| matches!(
            apply,
            crate::V2RemoteApply::Thread { thread_id, .. } if thread_id == "th_pending"
        )));
        // dirty 保留：bootstrap 建立 states 后由下轮 pass 正常推送
        assert!(manager
            .v2_dirty_thread_ids()
            .unwrap()
            .contains("th_pending"));
    }
}
