use super::*;

struct DownloadedNotePayload {
    content: Option<Vec<u8>>,
    attachments: Vec<crate::v2::V2RemoteAttachment>,
}

impl SyncManager {
    async fn download_note_payload(
        &self,
        access_token: &str,
        note_id: &str,
        content_hash: Option<&str>,
        deleted: bool,
        attachment_manifest: &[crate::v2::V2Attachment],
    ) -> Result<DownloadedNotePayload, SyncError> {
        let content = if deleted {
            None
        } else if let Some(hash) = content_hash {
            Some(
                self.download_verified_v2_blob(access_token, note_id, hash)
                    .await?,
            )
        } else {
            None
        };

        let mut attachments = Vec::new();
        if !deleted {
            attachments.reserve(attachment_manifest.len());
            for metadata in attachment_manifest {
                attachments.push(crate::v2::V2RemoteAttachment {
                    content: self
                        .download_verified_v2_blob(access_token, note_id, &metadata.content_hash)
                        .await?,
                    metadata: metadata.clone(),
                });
            }
        }

        Ok(DownloadedNotePayload {
            content,
            attachments,
        })
    }

    pub(super) async fn pull_v2_changes(
        &self,
        access_token: &str,
        cursor: i64,
        enabled: &HashSet<&str>,
        notebook_scope: Option<&str>,
        self_sync_seqs: &HashSet<i64>,
    ) -> Result<(Vec<V2RemoteApply>, i64, i64), SyncError> {
        let page = self.client.v2_changes(access_token, cursor, 100).await?;
        let next_cursor = page.cursor;
        let head_cursor = page.head_cursor;
        if next_cursor < cursor || (page.has_more && next_cursor == cursor) {
            return Err(SyncError::InvalidState(
                "cloud v2 changes cursor did not advance".into(),
            ));
        }
        let mut latest = HashMap::<(String, String), V2Change>::new();
        for change in page.changes {
            latest.insert(
                (change.entity_type.clone(), change.entity_id.clone()),
                change,
            );
        }
        let mut changes: Vec<_> = latest.into_values().collect();
        changes.sort_by_key(|change| change.sync_seq);
        let mut remote = Vec::new();
        for change in changes {
            // P0-1: 跳过本端刚 push 产生的 change，避免把自己的上传当成远端更新拉回
            // 覆盖本地（单端 echo）。其他设备的 change 不在 self_sync_seqs 中，正常 apply。
            if self_sync_seqs.contains(&change.sync_seq) {
                continue;
            }
            if change.entity_type == "notebook" {
                if notebook_scope.is_some_and(|scope| scope != change.entity_id) {
                    continue;
                }
                if !enabled.contains(change.entity_id.as_str()) {
                    continue;
                }
                remote.push(V2RemoteApply::Notebook {
                    notebook_id: change.entity_id,
                    name: change.name,
                    icon: change.icon,
                    sort_order: change.sort_order,
                    revision: change.revision,
                    sync_seq: change.sync_seq,
                    deleted: change.deleted,
                });
            } else if change.entity_type == "note" {
                let Some(notebook_id) = change.notebook_id else {
                    return Err(SyncError::InvalidState(
                        "cloud note change has no notebook".into(),
                    ));
                };
                if notebook_scope.is_some_and(|scope| scope != notebook_id) {
                    continue;
                }
                if !enabled.contains(notebook_id.as_str()) {
                    continue;
                }
                let filename = change.filename.ok_or_else(|| {
                    SyncError::InvalidState("cloud note change has no filename".into())
                })?;
                let payload = self
                    .download_note_payload(
                        access_token,
                        &change.entity_id,
                        change.content_hash.as_deref(),
                        change.deleted,
                        &change.attachments,
                    )
                    .await?;
                remote.push(V2RemoteApply::Note {
                    note_id: change.entity_id,
                    notebook_id,
                    filename,
                    content_hash: change.content_hash,
                    content: payload.content,
                    revision: change.revision,
                    sync_seq: change.sync_seq,
                    deleted: change.deleted,
                    attachments: payload.attachments,
                });
            } else if change.entity_type == "thread" {
                // thread 是全局实体：只在账户级同步物化（单笔记本 scope
                // 同步跳过，避免 notebook cursor 消费 thread change）。
                if notebook_scope.is_some() {
                    continue;
                }
                let agent_id = change.agent_id.clone().unwrap_or_default();
                let title = change.name.clone().unwrap_or_default();
                let content = if change.deleted {
                    None
                } else if let Some(hash) = change.content_hash.as_deref() {
                    Some(
                        self.download_verified_v2_blob(access_token, &change.entity_id, hash)
                            .await?,
                    )
                } else {
                    None
                };
                remote.push(V2RemoteApply::Thread {
                    thread_id: change.entity_id,
                    agent_id,
                    title,
                    content_hash: change.content_hash,
                    content,
                    revision: change.revision,
                    sync_seq: change.sync_seq,
                    deleted: change.deleted,
                });
            } else {
                // 前向容错：新版本服务端/对端可能产生旧客户端不认识的实体类型
                // （如未来的 thread）。跳过而不是炸掉整趟同步——否则一处未知
                // 实体会让旧客户端的笔记同步整体停摆。
                tracing::warn!(
                    entity_type = %change.entity_type,
                    entity_id = %change.entity_id,
                    sync_seq = change.sync_seq,
                    "skipping unknown cloud v2 entity type"
                );
                continue;
            }
        }
        Ok((remote, next_cursor, head_cursor))
    }

    pub(super) async fn remote_from_bootstrap(
        &self,
        access_token: &str,
        enabled: &HashSet<&str>,
        bootstrap: &V2Bootstrap,
    ) -> Result<Vec<V2RemoteApply>, SyncError> {
        let mut remote = Vec::new();
        for notebook in &bootstrap.notebooks {
            if enabled.contains(notebook.id.as_str()) {
                remote.push(V2RemoteApply::Notebook {
                    notebook_id: notebook.id.clone(),
                    name: Some(notebook.name.clone()),
                    icon: notebook.icon.clone(),
                    sort_order: Some(notebook.sort_order),
                    revision: notebook.revision.clone(),
                    sync_seq: notebook.sync_seq,
                    deleted: notebook.deleted,
                });
            }
        }
        for note in &bootstrap.notes {
            if !enabled.contains(note.notebook_id.as_str()) {
                continue;
            }
            let payload = self
                .download_note_payload(
                    access_token,
                    &note.id,
                    note.content_hash.as_deref(),
                    note.deleted,
                    &note.attachments,
                )
                .await?;
            remote.push(V2RemoteApply::Note {
                note_id: note.id.clone(),
                notebook_id: note.notebook_id.clone(),
                filename: note.filename.clone(),
                content_hash: note.content_hash.clone(),
                content: payload.content,
                revision: note.revision.clone(),
                sync_seq: note.sync_seq,
                deleted: note.deleted,
                attachments: payload.attachments,
            });
        }
        // threads 全量输出不分页（A1 服务端）；与笔记无关，不过 enabled 过滤。
        for thread in &bootstrap.threads {
            let content = if thread.deleted {
                None
            } else if let Some(hash) = thread.content_hash.as_deref() {
                Some(
                    self.download_verified_v2_blob(access_token, &thread.id, hash)
                        .await?,
                )
            } else {
                None
            };
            remote.push(V2RemoteApply::Thread {
                thread_id: thread.id.clone(),
                agent_id: thread.agent_id.clone(),
                title: thread.title.clone(),
                content_hash: thread.content_hash.clone(),
                content,
                revision: thread.revision.clone(),
                sync_seq: thread.sync_seq,
                deleted: thread.deleted,
            });
        }
        remote.sort_by_key(|change| match change {
            V2RemoteApply::Notebook { sync_seq, .. }
            | V2RemoteApply::Note { sync_seq, .. }
            | V2RemoteApply::Thread { sync_seq, .. } => *sync_seq,
        });
        Ok(remote)
    }

    pub(super) async fn download_verified_v2_blob(
        &self,
        access_token: &str,
        note_id: &str,
        expected_hash: &str,
    ) -> Result<Vec<u8>, SyncError> {
        let content = self
            .client
            .v2_download_blob(access_token, expected_hash)
            .await?;
        verify_v2_blob(note_id, expected_hash, content)
    }

    /// V2Bootstrap 反序列化必须忽略未知字段（未来服务端新增 threads 等数组时
    /// 旧客户端 bootstrap 不炸）。
    #[cfg(test)]
    pub(super) fn bootstrap_deserialization_ignores_unknown_fields() -> bool {
        let json = serde_json::json!({
            "protocolEpoch": 2,
            "cursor": 1,
            "serverTime": 2,
            "notebooks": [],
            "notes": [],
            "usage": { "usedBytes": 0, "quotaBytes": 1, "entitlementExpiresAt": null },
            // 模拟服务端未来新增的顶层实体（A0 期用 threads，A1 起为已知字段）
            "conversations": [{ "id": "cv_future", "messages": [] }]
        });
        serde_json::from_value::<crate::v2::V2Bootstrap>(json).is_ok()
    }
}

#[cfg(test)]
mod inbound_forward_compat_tests {
    use crate::{RefreshTokenStore, SyncManager};
    use std::collections::HashSet;
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

    /// 极简单次响应 HTTP mock：返回一页 changes（thread 未知实体 + notebook 正常）。
    async fn spawn_changes_mock(body: &'static str) -> String {
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

    /// A0 前向容错回归：changes 流里出现旧客户端不认识的实体类型（A0 期用
    /// thread 模拟，A2 起 thread 已物化——改用未来实体 conversation）时，
    /// 必须跳过该条并继续处理其余 change，而不是让整趟同步失败
    /// （历史上这里直接 Err，一处未知实体会让旧客户端的笔记同步整体停摆）。
    /// 同页的 thread（A2 已知实体）应正常物化。
    #[tokio::test]
    async fn pull_v2_changes_skips_unknown_entity_types() {
        let body = r#"{"data":{"protocolEpoch":2,"cursor":6,"headCursor":6,"hasMore":false,"changes":[
            {"syncSeq":3,"entityType":"conversation","entityId":"cv_future","notebookId":null,"revision":"rev_3","kind":"put","createdAt":1,"deleted":false,"attachments":[]},
            {"syncSeq":4,"entityType":"thread","entityId":"th_known","notebookId":null,"revision":"rev_4","kind":"delete","createdAt":1,"deleted":true,"attachments":[]},
            {"syncSeq":5,"entityType":"notebook","entityId":"nb_ok","notebookId":null,"revision":"rev_5","kind":"put","createdAt":1,"name":"OK","sortOrder":0,"deleted":false,"attachments":[]}
        ]}}"#;
        let base = spawn_changes_mock(body).await;
        let manager = SyncManager::new_with_session_dependencies(
            &base,
            std::env::temp_dir().join(format!("flowix-sync-a0-{}.db", std::process::id())),
            Arc::new(NullTokenStore),
            Arc::new(|_| {}),
        )
        .unwrap();

        let enabled: HashSet<&str> = HashSet::from(["nb_ok"]);
        let result = manager
            .pull_v2_changes("at_test", 0, &enabled, None, &HashSet::new())
            .await;
        let (remote, next_cursor, head_cursor) = result.expect("unknown entity type must not fail the pull");
        assert_eq!(next_cursor, 6);
        assert_eq!(head_cursor, 6);
        // conversation 跳过；thread（delete，无需 blob）与 notebook 物化
        assert_eq!(remote.len(), 2, "unknown skipped, thread + notebook materialize");
        let mut thread_seen = false;
        for apply in &remote {
            match apply {
                crate::V2RemoteApply::Notebook { notebook_id, name, .. } => {
                    assert_eq!(notebook_id, "nb_ok");
                    assert_eq!(name.as_deref(), Some("OK"));
                }
                crate::V2RemoteApply::Thread { thread_id, deleted, content, .. } => {
                    assert_eq!(thread_id, "th_known");
                    assert!(*deleted);
                    assert!(content.is_none(), "deleted thread has no blob download");
                    thread_seen = true;
                }
                other => panic!("unexpected apply variant: {other:?}"),
            }
        }
        assert!(thread_seen);
    }

    #[test]
    fn bootstrap_ignores_unknown_top_level_fields() {
        assert!(SyncManager::bootstrap_deserialization_ignores_unknown_fields());
    }
}
