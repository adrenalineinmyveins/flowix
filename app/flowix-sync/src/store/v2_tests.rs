use super::*;
use crate::models::CloudUser;
use crate::v2::{
    V2CloudAccount, V2EntityType, V2FreezeOperation, V2NotePut, V2NoteState, V2OperationData,
    V2OperationKind, V2PushOperation, V2RemoteApply, PROTOCOL_EPOCH,
};

#[test]
fn v2_state_uses_one_account_cursor_and_same_notebook_id() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let account = V2CloudAccount {
        user: CloudUser {
            id: "usr_1".into(),
            email: "user@example.com".into(),
            display_name: "User".into(),
            system_role: "user".into(),
        },
        protocol_epoch: PROTOCOL_EPOCH,
    };
    store.save_v2_account(&account).unwrap();
    assert_eq!(store.v2_account().unwrap(), Some(account));
    assert_eq!(store.v2_cursor().unwrap(), 0);

    let enabled = store
        .set_v2_notebook("nb_same_local_and_cloud", true)
        .unwrap();
    assert_eq!(enabled.notebook_id, "nb_same_local_and_cloud");
    assert!(enabled.enabled);
    assert!(enabled.bootstrap_required);
    store
        .complete_v2_notebook_bootstrap("nb_same_local_and_cloud")
        .unwrap();
    assert!(!store.v2_notebooks(true).unwrap()[0].bootstrap_required);

    store
        .set_v2_notebook("nb_same_local_and_cloud", false)
        .unwrap();
    let reenabled = store
        .set_v2_notebook("nb_same_local_and_cloud", true)
        .unwrap();
    assert!(reenabled.bootstrap_required);

    store.commit_v2_cursor(12, 100).unwrap();
    assert_eq!(store.v2_cursor().unwrap(), 12);
    assert!(store.commit_v2_cursor(11, 101).is_err());
}

#[test]
fn scoped_notebook_cursors_do_not_advance_each_other() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    store.set_v2_notebook("nb_a", true).unwrap();
    store.set_v2_notebook("nb_b", true).unwrap();

    store
        .commit_v2_notebook_sync_report("nb_a", &[], 10, &["nb_a".into()], 100)
        .unwrap();
    assert_eq!(store.v2_notebook_cursor("nb_a").unwrap(), 10);
    assert_eq!(store.v2_notebook_cursor("nb_b").unwrap(), 0);
    assert_eq!(store.v2_cursor().unwrap(), 0);

    store
        .commit_v2_notebook_sync_report("nb_b", &[], 20, &["nb_b".into()], 200)
        .unwrap();
    assert_eq!(store.v2_notebook_cursor("nb_a").unwrap(), 10);
    assert_eq!(store.v2_notebook_cursor("nb_b").unwrap(), 20);

    store.commit_v2_sync_report(&[], 30, &[], 300).unwrap();
    assert_eq!(store.v2_cursor().unwrap(), 30);
    assert_eq!(store.v2_notebook_cursor("nb_a").unwrap(), 30);
    assert_eq!(store.v2_notebook_cursor("nb_b").unwrap(), 30);
}

#[test]
fn v2_dirty_generation_does_not_lose_an_edit_that_arrives_during_upload() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let (first, first_changed) = store
        .mark_v2_dirty_with_change(
            V2EntityType::Note,
            "abc12345",
            Some("nb_1"),
            V2OperationKind::Put,
            "hash-a",
            10,
        )
        .unwrap();
    assert!(first_changed);
    let (repeated_scan, repeated_changed) = store
        .mark_v2_dirty_with_change(
            V2EntityType::Note,
            "abc12345",
            Some("nb_1"),
            V2OperationKind::Put,
            "hash-a",
            11,
        )
        .unwrap();
    assert!(!repeated_changed);
    assert_eq!(repeated_scan.generation, first.generation);
    let frozen = store
        .freeze_v2_operation(V2FreezeOperation {
            operation_id: "op_generation_1",
            entity_type: V2EntityType::Note,
            entity_id: "abc12345",
            generation: first.generation,
            operation_kind: V2OperationKind::Put,
            base_revision: Some("rev_1"),
            payload_json: r#"{"contentHash":"hash-a"}"#,
        })
        .unwrap();
    assert_eq!(frozen.generation, 1);
    assert_eq!(store.v2_inflight_due(0).unwrap().len(), 1);

    let (second, second_changed) = store
        .mark_v2_dirty_with_change(
            V2EntityType::Note,
            "abc12345",
            Some("nb_1"),
            V2OperationKind::Put,
            "hash-b",
            20,
        )
        .unwrap();
    assert!(second_changed);
    assert_eq!(second.generation, 2);

    let operation = V2PushOperation::NotePut {
        operation_id: frozen.operation_id.clone(),
        base_revision: Some("rev_1".into()),
        note: V2NotePut {
            id: "abc12345".into(),
            notebook_id: "nb_1".into(),
            filename: "abc12345.md".into(),
            file_kind: "markdown".into(),
            content_hash: "hash-a".into(),
            size_bytes: 6,
            attachments: Vec::new(),
        },
    };
    let data = V2OperationData {
        entity_type: "note".into(),
        entity_id: "abc12345".into(),
        revision: "rev_2".into(),
        sync_seq: 2,
        deleted: false,
        resolution: "fast_forward".into(),
        base_revision: Some("rev_1".into()),
        replaced_revision: Some("rev_1".into()),
        content_hash: Some("hash-a".into()),
        source_revision: None,
        source_sync_seq: None,
    };

    store
        .acknowledge_v2_operation(
            &frozen.operation_id,
            V2EntityType::Note,
            "abc12345",
            frozen.generation,
            &operation,
            &data,
        )
        .unwrap();
    assert_eq!(
        store.v2_note_state("abc12345").unwrap(),
        Some(V2NoteState {
            note_id: "abc12345".into(),
            notebook_id: "nb_1".into(),
            revision: "rev_2".into(),
            content_hash: Some("hash-a".into()),
            filename: "abc12345.md".into(),
            deleted: false,
            last_seq: 2,
            attachments: Vec::new(),
        })
    );
    let connection = store.open().unwrap();
    let generation = connection
        .query_row(
            "SELECT generation FROM v2_dirty_entities WHERE entity_type = 'note' AND entity_id = 'abc12345'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(generation, 2);
}

#[test]
fn v2_note_state_tracks_server_revision_not_client_time() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let state = V2NoteState {
        note_id: "abc12345".into(),
        notebook_id: "nb_1".into(),
        revision: "rev_c".into(),
        content_hash: Some("hash".into()),
        filename: "abc12345.md".into(),
        deleted: false,
        last_seq: 12,
        attachments: Vec::new(),
    };
    store.save_v2_note_state(&state).unwrap();
    assert_eq!(store.v2_note_state("abc12345").unwrap(), Some(state));
}

#[test]
fn pending_move_survives_reopen_and_acknowledges_both_heads() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("sync.db");
    let store = SyncStore::new(&database).unwrap();
    let notebook = "nb_1";
    let old = "folder/old.md";
    let new = "folder/new.md";
    let old_id = crate::v2::v2_path_note_id(notebook, old);
    let new_id = crate::v2::v2_path_note_id(notebook, new);
    store
        .save_v2_note_state(&V2NoteState {
            note_id: old_id.clone(),
            notebook_id: notebook.into(),
            revision: "rev_2".into(),
            content_hash: Some("hash-a".into()),
            filename: old.into(),
            deleted: false,
            last_seq: 2,
            attachments: Vec::new(),
        })
        .unwrap();
    assert!(store.enqueue_v2_move(notebook, old, new).unwrap());
    assert!(!store.enqueue_v2_move(notebook, old, new).unwrap());
    drop(store);
    let reopened = SyncStore::new(&database).unwrap();
    let queued = reopened.v2_pending_moves().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].base_revision, "rev_2");
    let data = V2OperationData {
        entity_type: "note".into(),
        entity_id: new_id.clone(),
        revision: "rev_4".into(),
        sync_seq: 4,
        deleted: false,
        resolution: "fast_forward".into(),
        base_revision: Some("rev_2".into()),
        replaced_revision: None,
        content_hash: Some("hash-a".into()),
        source_revision: Some("rev_3".into()),
        source_sync_seq: Some(3),
    };
    reopened.acknowledge_v2_move(&queued[0], &data).unwrap();
    assert!(reopened.v2_pending_moves().unwrap().is_empty());
    let source = reopened.v2_note_state(&old_id).unwrap().unwrap();
    let target = reopened.v2_note_state(&new_id).unwrap().unwrap();
    assert!(source.deleted);
    assert_eq!(source.revision, "rev_3");
    assert_eq!(target.revision, "rev_4");
    assert_eq!(target.content_hash.as_deref(), Some("hash-a"));
}

#[test]
fn moving_back_reuses_the_prior_path_tombstone() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let notebook = "nb_1";
    let old = "folder/old.md";
    let current = "folder/current.md";
    let old_id = crate::v2::v2_path_note_id(notebook, old);
    let current_id = crate::v2::v2_path_note_id(notebook, current);
    store
        .save_v2_note_state(&V2NoteState {
            note_id: old_id.clone(),
            notebook_id: notebook.into(),
            revision: "rev_3".into(),
            content_hash: None,
            filename: old.into(),
            deleted: true,
            last_seq: 3,
            attachments: Vec::new(),
        })
        .unwrap();
    store
        .save_v2_note_state(&V2NoteState {
            note_id: current_id.clone(),
            notebook_id: notebook.into(),
            revision: "rev_4".into(),
            content_hash: Some("hash-current".into()),
            filename: current.into(),
            deleted: false,
            last_seq: 4,
            attachments: Vec::new(),
        })
        .unwrap();

    assert!(store.enqueue_v2_move(notebook, current, old).unwrap());
    let queued = store.v2_pending_moves().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].from_note_id, current_id);
    assert_eq!(queued[0].to_note_id, old_id);
}

#[test]
fn rejected_move_is_removed_only_with_a_required_bootstrap() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let notebook = "nb_1";
    let from = "old.md";
    let to = "new.md";
    store.set_v2_notebook(notebook, true).unwrap();
    store.complete_v2_notebook_bootstrap(notebook).unwrap();
    store
        .save_v2_note_state(&V2NoteState {
            note_id: crate::v2::v2_path_note_id(notebook, from),
            notebook_id: notebook.into(),
            revision: "rev_2".into(),
            content_hash: Some("base-hash".into()),
            filename: from.into(),
            deleted: false,
            last_seq: 2,
            attachments: Vec::new(),
        })
        .unwrap();
    assert!(store.enqueue_v2_move(notebook, from, to).unwrap());
    let movement = store.v2_pending_moves().unwrap().remove(0);

    store.discard_v2_move_after_conflict(&movement).unwrap();

    assert!(store.v2_pending_moves().unwrap().is_empty());
    assert!(store.v2_notebooks(true).unwrap()[0].bootstrap_required);
}

#[test]
fn v2_report_commits_heads_bootstrap_and_cursor_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    store.set_v2_notebook("nb_1", true).unwrap();
    let remote = vec![
        V2RemoteApply::Notebook {
            notebook_id: "nb_1".into(),
            name: Some("Notes".into()),
            icon: None,
            sort_order: Some(0),
            revision: "rev_1".into(),
            sync_seq: 1,
            deleted: false,
        },
        V2RemoteApply::Note {
            note_id: "abc12345".into(),
            notebook_id: "nb_1".into(),
            filename: "abc12345.md".into(),
            content_hash: Some("hash".into()),
            content: Some(b"memo".to_vec()),
            revision: "rev_2".into(),
            sync_seq: 2,
            deleted: false,
            attachments: Vec::new(),
        },
    ];
    store
        .commit_v2_sync_report(&remote, 2, &["nb_1".into()], 100)
        .unwrap();
    assert_eq!(store.v2_cursor().unwrap(), 2);
    assert_eq!(
        store.v2_note_state("abc12345").unwrap().unwrap().revision,
        "rev_2"
    );
    assert_eq!(
        store.v2_notebook_state("nb_1").unwrap().unwrap().revision,
        "rev_1"
    );
    assert!(!store.v2_notebooks(true).unwrap()[0].bootstrap_required);
}

#[test]
fn bootstrap_pages_keep_cursor_and_requirement_until_final_page() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    store.set_v2_notebook("nb_1", true).unwrap();
    let first = V2RemoteApply::Note {
        note_id: "first".into(),
        notebook_id: "nb_1".into(),
        filename: "first.md".into(),
        content_hash: Some("hash-first".into()),
        content: Some(b"first".to_vec()),
        revision: "rev_1".into(),
        sync_seq: 1,
        deleted: false,
        attachments: Vec::new(),
    };
    store.commit_v2_sync_report(&[first], 0, &[], 100).unwrap();
    assert_eq!(store.v2_cursor().unwrap(), 0);
    assert!(store.v2_notebooks(true).unwrap()[0].bootstrap_required);
    assert!(store.v2_note_state("first").unwrap().is_some());

    let second = V2RemoteApply::Note {
        note_id: "second".into(),
        notebook_id: "nb_1".into(),
        filename: "second.md".into(),
        content_hash: Some("hash-second".into()),
        content: Some(b"second".to_vec()),
        revision: "rev_2".into(),
        sync_seq: 2,
        deleted: false,
        attachments: Vec::new(),
    };
    store
        .commit_v2_sync_report(&[second], 2, &["nb_1".into()], 101)
        .unwrap();
    assert_eq!(store.v2_cursor().unwrap(), 2);
    assert!(!store.v2_notebooks(true).unwrap()[0].bootstrap_required);
    assert!(store.v2_note_state("second").unwrap().is_some());
}

#[test]
fn switching_users_clears_user_scoped_sync_state() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let account = |id: &str| V2CloudAccount {
        user: CloudUser {
            id: id.into(),
            email: format!("{id}@example.com"),
            display_name: id.into(),
            system_role: "user".into(),
        },
        protocol_epoch: PROTOCOL_EPOCH,
    };
    store.save_v2_account(&account("usr_a")).unwrap();
    store.set_v2_notebook("nb_a", true).unwrap();
    store.commit_v2_cursor(9, 100).unwrap();
    store.save_v2_account(&account("usr_b")).unwrap();

    assert_eq!(store.v2_cursor().unwrap(), 0);
    assert!(store.v2_notebooks(false).unwrap().is_empty());
    assert_eq!(store.v2_account().unwrap().unwrap().user.id, "usr_b");
}

#[test]
fn clearing_account_removes_user_scoped_sync_state() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    store
        .save_v2_account(&V2CloudAccount {
            user: CloudUser {
                id: "usr_a".into(),
                email: "a@example.com".into(),
                display_name: "A".into(),
                system_role: "user".into(),
            },
            protocol_epoch: PROTOCOL_EPOCH,
        })
        .unwrap();
    store.set_v2_notebook("nb_a", true).unwrap();
    store.commit_v2_cursor(9, 100).unwrap();

    store.clear_v2_account().unwrap();

    assert!(store.v2_account().unwrap().is_none());
    assert_eq!(store.v2_cursor().unwrap(), 0);
    assert!(store.v2_notebooks(false).unwrap().is_empty());
}

// ── 方案 A2：thread 实体的迁移与引擎管线 store 层测试 ──────────

/// R2 迁移回归：旧 CHECK(entity_type IN ('notebook','note')) 库升级后，
/// 两表数据（含 inflight）完整保留且接受 'thread'。
#[test]
fn thread_migration_rebuilds_check_constraints_preserving_inflight_data() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("sync.db");
    {
        // 手造旧 schema 库（A2 之前的 DDL）
        let legacy = rusqlite::Connection::open(&db_path).unwrap();
        legacy
            .execute_batch(
                r#"
                CREATE TABLE v2_dirty_entities (
                    entity_type TEXT NOT NULL CHECK(entity_type IN ('notebook', 'note')),
                    entity_id TEXT NOT NULL,
                    notebook_id TEXT,
                    generation INTEGER NOT NULL CHECK(generation > 0),
                    operation_kind TEXT NOT NULL CHECK(operation_kind IN ('put', 'delete')),
                    fingerprint TEXT NOT NULL,
                    detected_at INTEGER NOT NULL,
                    PRIMARY KEY(entity_type, entity_id)
                );
                CREATE TABLE v2_inflight_operations (
                    operation_id TEXT PRIMARY KEY,
                    entity_type TEXT NOT NULL CHECK(entity_type IN ('notebook', 'note')),
                    entity_id TEXT NOT NULL,
                    generation INTEGER NOT NULL CHECK(generation > 0),
                    operation_kind TEXT NOT NULL CHECK(operation_kind IN ('put', 'delete')),
                    base_revision TEXT,
                    payload_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    UNIQUE(entity_type, entity_id, generation)
                );
                CREATE TABLE v2_retry_state (
                    operation_id TEXT PRIMARY KEY REFERENCES v2_inflight_operations(operation_id) ON DELETE CASCADE,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_retry_at INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO v2_dirty_entities VALUES ('note', 'note_inflight', 'nb_1', 3, 'put', 'fp', 100);
                INSERT INTO v2_inflight_operations VALUES ('op_legacy', 'note', 'note_inflight', 3, 'put', 'rev_a', '{}', 100);
                INSERT INTO v2_retry_state (operation_id, attempts, next_retry_at, updated_at) VALUES ('op_legacy', 2, 50, 100);
                "#,
            )
            .unwrap();
    }
    // open 触发 initialize_v2_schema → CHECK 重建迁移
    let store = SyncStore::new(&db_path).unwrap();

    // 旧数据保留
    let inflight = store.v2_inflight_due(i64::MAX).unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(inflight[0].operation_id, "op_legacy");
    assert_eq!(inflight[0].attempts, 2);
    let dirty = store.v2_dirty_entities().unwrap();
    assert_eq!(dirty.len(), 1);
    assert_eq!(dirty[0].entity_id, "note_inflight");

    // thread 实体可写入（迁移后 CHECK 接受）
    store
        .mark_v2_dirty_with_change(
            V2EntityType::Thread,
            "th_migrated",
            None,
            V2OperationKind::Put,
            "fp-thread",
            200,
        )
        .unwrap();
    assert!(store
        .v2_dirty_entities()
        .unwrap()
        .iter()
        .any(|item| item.entity_type == V2EntityType::Thread && item.entity_id == "th_migrated"));

    // 二次 open 幂等（不再触发迁移）
    let store_again = SyncStore::new(&db_path).unwrap();
    assert_eq!(store_again.v2_dirty_entities().unwrap().len(), 2);
}

#[test]
fn thread_put_acknowledgement_and_commit_update_thread_state() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    let operation_id = crate::v2::new_v2_operation_id();
    store
        .mark_v2_dirty_with_change(
            V2EntityType::Thread,
            "th_ack",
            None,
            V2OperationKind::Put,
            "fp",
            100,
        )
        .unwrap();
    let dirty = store
        .v2_dirty_entities()
        .unwrap()
        .into_iter()
        .find(|item| item.entity_id == "th_ack")
        .unwrap();
    let payload = serde_json::to_string(&V2PushOperation::ThreadPut {
        operation_id: operation_id.clone(),
        base_revision: None,
        thread: crate::v2::V2ThreadPut {
            id: "th_ack".into(),
            agent_id: "agent_loop".into(),
            title: "会话".into(),
            content_hash: "hash_1".into(),
            size_bytes: 10,
        },
    })
    .unwrap();
    store
        .freeze_v2_operation(V2FreezeOperation {
            operation_id: &operation_id,
            entity_type: V2EntityType::Thread,
            entity_id: "th_ack",
            generation: dirty.generation,
            operation_kind: V2OperationKind::Put,
            base_revision: None,
            payload_json: &payload,
        })
        .unwrap();

    let operation: V2PushOperation = serde_json::from_str(&payload).unwrap();
    match &operation {
        V2PushOperation::ThreadPut { thread, .. } => {
            store
                .acknowledge_v2_operation(
                    &operation_id,
                    V2EntityType::Thread,
                    "th_ack",
                    dirty.generation,
                    &operation,
                    &V2OperationData {
                        entity_type: "thread".into(),
                        entity_id: thread.id.clone(),
                        revision: "rev_t1".into(),
                        sync_seq: 7,
                        deleted: false,
                        resolution: "created".into(),
                        base_revision: None,
                        replaced_revision: None,
                        content_hash: Some(thread.content_hash.clone()),
                        source_revision: None,
                        source_sync_seq: None,
                    },
                )
                .unwrap();
        }
        other => panic!("unexpected operation: {other:?}"),
    }
    let state = store.v2_thread_state("th_ack").unwrap().unwrap();
    assert_eq!(state.revision, "rev_t1");
    assert_eq!(state.content_hash.as_deref(), Some("hash_1"));
    assert_eq!(state.title, "会话");
    assert!(!state.deleted);
    assert_eq!(state.last_seq, 7);
    // ack 清空队列
    assert!(store
        .v2_dirty_entities()
        .unwrap()
        .iter()
        .all(|item| item.entity_id != "th_ack"));
    assert!(store.v2_inflight_due(i64::MAX).unwrap().is_empty());
}

#[test]
fn thread_remote_apply_commit_updates_state_and_bootstrap_flag() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    assert!(store.v2_threads_bootstrap_required().unwrap(), "初始需要 bootstrap");

    let remote = vec![V2RemoteApply::Thread {
        thread_id: "th_pull".into(),
        agent_id: "agent_loop".into(),
        title: "拉取的会话".into(),
        content_hash: Some("hash_p".into()),
        content: Some(br#"{"version":1}"#.to_vec()),
        revision: "rev_p1".into(),
        sync_seq: 5,
        deleted: false,
    }];
    store
        .commit_v2_sync_report_with_threads(&remote, 5, &[], 100, true)
        .unwrap();
    let state = store.v2_thread_state("th_pull").unwrap().unwrap();
    assert_eq!(state.revision, "rev_p1");
    assert!(!store.v2_threads_bootstrap_required().unwrap(), "bootstrap 后清标志");
}

#[test]
fn thread_conflict_resolution_paths_manage_dirty_and_state() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncStore::new(temp.path().join("sync.db")).unwrap();
    // 造一个 inflight thread put
    let operation_id = crate::v2::new_v2_operation_id();
    store
        .mark_v2_dirty_with_change(
            V2EntityType::Thread,
            "th_c",
            None,
            V2OperationKind::Put,
            "fp",
            100,
        )
        .unwrap();
    let dirty = store
        .v2_dirty_entities()
        .unwrap()
        .into_iter()
        .find(|item| item.entity_id == "th_c")
        .unwrap();
    let payload = serde_json::to_string(&V2PushOperation::ThreadPut {
        operation_id: operation_id.clone(),
        base_revision: Some("rev_old".into()),
        thread: crate::v2::V2ThreadPut {
            id: "th_c".into(),
            agent_id: "a".into(),
            title: "t".into(),
            content_hash: "h".into(),
            size_bytes: 1,
        },
    })
    .unwrap();
    store
        .freeze_v2_operation(V2FreezeOperation {
            operation_id: &operation_id,
            entity_type: V2EntityType::Thread,
            entity_id: "th_c",
            generation: dirty.generation,
            operation_kind: V2OperationKind::Put,
            base_revision: Some("rev_old"),
            payload_json: &payload,
        })
        .unwrap();
    let item = store.v2_inflight_due(i64::MAX).unwrap().remove(0);

    // 正常 409：丢 inflight、保留 dirty
    store.defer_thread_conflict_to_resync(&item).unwrap();
    assert!(store.v2_inflight_due(i64::MAX).unwrap().is_empty());
    assert!(store
        .v2_dirty_entities()
        .unwrap()
        .iter()
        .any(|d| d.entity_id == "th_c"));

    // 远端已删 + 本地 delete：按远端头结算
    store
        .mark_v2_dirty_with_change(
            V2EntityType::Thread,
            "th_d",
            None,
            V2OperationKind::Delete,
            "fp",
            100,
        )
        .unwrap();
    let op_d = crate::v2::new_v2_operation_id();
    let payload_d = serde_json::to_string(&V2PushOperation::ThreadDelete {
        operation_id: op_d.clone(),
        base_revision: Some("rev_x".into()),
        thread_id: "th_d".into(),
    })
    .unwrap();
    store
        .freeze_v2_operation(V2FreezeOperation {
            operation_id: &op_d,
            entity_type: V2EntityType::Thread,
            entity_id: "th_d",
            generation: 1,
            operation_kind: V2OperationKind::Delete,
            base_revision: Some("rev_x"),
            payload_json: &payload_d,
        })
        .unwrap();
    let item_d = store.v2_inflight_due(i64::MAX).unwrap().remove(0);
    store
        .settle_v2_thread_delete_as_remote_head(&item_d, "rev_deleted_remote", 9)
        .unwrap();
    let state = store.v2_thread_state("th_d").unwrap().unwrap();
    assert!(state.deleted);
    assert_eq!(state.revision, "rev_deleted_remote");
    assert!(store
        .v2_dirty_entities()
        .unwrap()
        .iter()
        .all(|d| d.entity_id != "th_d"));

    // 远端已删 + 本地 put：清基线走复活
    store
        .save_v2_thread_state(&crate::v2::V2ThreadState {
            thread_id: "th_c".into(),
            revision: "rev_old".into(),
            content_hash: Some("h".into()),
            title: "t".into(),
            deleted: false,
            last_seq: 1,
        })
        .unwrap();
    store.reset_v2_thread_for_recreate(&item).unwrap();
    assert!(store.v2_thread_state("th_c").unwrap().is_none(), "基线已清空");
    assert!(store
        .v2_dirty_entities()
        .unwrap()
        .iter()
        .any(|d| d.entity_id == "th_c"), "dirty 保留供复活重推");
}
