//! V2 同步核心：status / changes / bootstrap / history / push。
//!
//! 服务端语义（坑清单，实测对齐 flowix-sync 引擎）：
//! 1. push 逐操作返回；冲突在 **HTTP 200 响应体内**（ok:false + status:409 +
//!    error.code=REVISION_CONFLICT + error.details.currentRevision）
//! 2. `V2BootstrapNote.syncSeq` 必填 → 实体表持久化每实体最后 sync_seq
//! 3. notebook.put 走 **create-or-adopt**（base=null 且已存在 → 接受）：
//!    移动端认领云端笔记本（set_v2_notebook_enabled）以 base=null 推送，
//!    严格检查会把客户端逼进无解路径（notebook 无三方合并）
//! 4. note 内容走内容寻址 blob：push 只带 contentHash，服务端校验 blob 存在

use crate::db::{record_change, user_head, user_usage, ChangeRow, QUOTA_BYTES};
use crate::{api_error, data_reply, SharedState};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use flowix_sync::{
    V2Attachment, V2Bootstrap, V2BootstrapNote, V2BootstrapNotebook, V2BootstrapThread, V2Change,
    V2ChangesPage, V2History, V2HistoryRevision, V2OperationData, V2OperationError,
    V2OperationResult, V2PushOperation, V2PushResult, V2SyncStatus, V2Usage,
};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Deserialize;

const EPOCH: i64 = flowix_sync::PROTOCOL_EPOCH;

pub async fn products() -> axum::response::Response {
    // 自建版无付费商品；客户端按 Vec 解析空数组
    data_reply(Vec::<serde_json::Value>::new())
}

pub async fn checkout_unsupported() -> axum::response::Response {
    api_error(StatusCode::NOT_IMPLEMENTED, "NOT_SUPPORTED", "自建版暂不支持在线购买")
}

pub async fn status(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CursorQuery>,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let cursor = query.cursor.unwrap_or(0);
    let data = state.db.with(|connection| {
        let head = user_head(connection, &user_id);
        Ok::<_, String>(V2SyncStatus {
            protocol_epoch: EPOCH,
            cursor,
            head_cursor: head,
            has_changes: head > cursor,
            server_time: crate::now_ms(),
        })
    });
    match data {
        Ok(data) => data_reply(data),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

#[derive(Deserialize)]
pub struct CursorQuery {
    pub cursor: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn changes(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CursorQuery>,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let cursor = query.cursor.unwrap_or(0);
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let data = state.db.with(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT * FROM changes WHERE user_id = ?1 AND sync_seq > ?2
                 ORDER BY sync_seq ASC LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(rusqlite::params![user_id, cursor, limit], change_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let head = user_head(connection, &user_id);
        let last_seq = rows.last().map(|row| row.sync_seq).unwrap_or(cursor);
        Ok::<_, String>(V2ChangesPage {
            protocol_epoch: EPOCH,
            cursor: last_seq,
            head_cursor: head,
            has_more: last_seq < head,
            changes: rows,
        })
    });
    match data {
        Ok(data) => data_reply(data),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

fn change_from_row(row: &Row<'_>) -> rusqlite::Result<V2Change> {
    Ok(V2Change {
        sync_seq: row.get("sync_seq")?,
        entity_type: row.get("entity_type")?,
        entity_id: row.get("entity_id")?,
        notebook_id: row.get("notebook_id")?,
        revision: row.get("revision")?,
        kind: row.get("kind")?,
        created_at: row.get("created_at")?,
        name: row.get("name")?,
        icon: row.get("icon")?,
        sort_order: row.get("sort_order")?,
        filename: row.get("filename")?,
        agent_id: row.get("agent_id")?,
        content_hash: row.get("content_hash")?,
        size_bytes: row.get("size_bytes")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        attachments: serde_json::from_str(
            &row.get::<_, String>("attachments").unwrap_or_else(|_| "[]".into()),
        )
        .unwrap_or_default(),
    })
}

fn parse_page_token(token: &str) -> Option<(i64, Option<i64>)> {
    let rest = token.strip_prefix('o')?;
    let (offset, cursor) = rest.split_once('c')?;
    Some((offset.parse().ok()?, Some(cursor.parse().ok()?)))
}

fn bootstrap_notebook_from_row(row: &Row<'_>) -> rusqlite::Result<V2BootstrapNotebook> {
    Ok(V2BootstrapNotebook {
        id: row.get("id")?,
        name: row.get("name")?,
        icon: row.get("icon")?,
        sort_order: row.get("sort_order")?,
        revision: row.get("revision")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        sync_seq: row.get("sync_seq")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn bootstrap_note_from_row(row: &Row<'_>) -> rusqlite::Result<V2BootstrapNote> {
    Ok(V2BootstrapNote {
        id: row.get("id")?,
        notebook_id: row.get("notebook_id")?,
        filename: row.get("filename")?,
        moved_from_note_id: row.get("moved_from_note_id")?,
        revision: row.get("revision")?,
        content_hash: row.get("content_hash")?,
        size_bytes: row.get("size_bytes")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        sync_seq: row.get("sync_seq")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        attachments: serde_json::from_str(
            &row.get::<_, String>("attachments").unwrap_or_else(|_| "[]".into()),
        )
        .unwrap_or_default(),
    })
}

fn bootstrap_thread_from_row(row: &Row<'_>) -> rusqlite::Result<V2BootstrapThread> {
    Ok(V2BootstrapThread {
        id: row.get("id")?,
        agent_id: row.get("agent_id")?,
        title: row.get("title")?,
        revision: row.get("revision")?,
        content_hash: row.get("content_hash")?,
        size_bytes: row.get("size_bytes")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        sync_seq: row.get("sync_seq")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapQuery {
    pub limit: Option<i64>,
    pub page_token: Option<String>,
}

pub async fn bootstrap(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<BootstrapQuery>,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let limit = query.limit.unwrap_or(10).clamp(1, 1000);
    // pageToken 编码「偏移 + 快照 cursor」，保证跨页快照稳定
    let (offset, snapshot_cursor) = match query.page_token.as_deref() {
        None => (0i64, None),
        Some(token) => match parse_page_token(token) {
            Some(parsed) => parsed,
            None => {
                return api_error(StatusCode::BAD_REQUEST, "INVALID_PAGE_TOKEN", "分页令牌无效")
            }
        },
    };
    let data = state.db.with(|connection| {
        let snapshot_cursor = snapshot_cursor.unwrap_or_else(|| user_head(connection, &user_id));
        let mut notebook_statement = connection
            .prepare("SELECT * FROM notebooks WHERE user_id = ?1 ORDER BY sort_order, created_at")
            .map_err(|e| e.to_string())?;
        let notebooks = notebook_statement
            .query_map([&user_id], bootstrap_notebook_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        // limit+1 探测 hasMore
        let mut note_statement = connection
            .prepare(
                "SELECT * FROM notes WHERE user_id = ?1 ORDER BY created_at, id
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| e.to_string())?;
        let probe = note_statement
            .query_map(rusqlite::params![user_id, limit + 1, offset], bootstrap_note_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let has_more = probe.len() as i64 > limit;
        let notes: Vec<V2BootstrapNote> = probe.into_iter().take(limit as usize).collect();
        // thread 数量级远小于 note，bootstrap 全量输出不分页
        let mut thread_statement = connection
            .prepare("SELECT * FROM threads WHERE user_id = ?1 ORDER BY created_at, id")
            .map_err(|e| e.to_string())?;
        let threads = thread_statement
            .query_map([&user_id], bootstrap_thread_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let (used_bytes, _) = user_usage(connection, &user_id);
        Ok::<_, String>(V2Bootstrap {
            protocol_epoch: EPOCH,
            cursor: snapshot_cursor,
            server_time: crate::now_ms(),
            notebooks,
            notes,
            threads,
            usage: V2Usage {
                used_bytes,
                quota_bytes: QUOTA_BYTES,
                entitlement_expires_at: None,
            },
            next_page_token: has_more.then(|| format!("o{}c{}", offset + limit, snapshot_cursor)),
        })
    });
    match data {
        Ok(data) => data_reply(data),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryQuery {
    pub note_id: String,
    pub limit: Option<i64>,
    pub before_seq: Option<i64>,
}

pub async fn history(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let data = state.db.with(|connection| {
        // thread 历史：th_ 前缀路由（方案 A：id 空间隔离防碰撞），
        // title 复用 V2HistoryRevision.relative_path 字段输出
        if query.note_id.starts_with("th_") {
            let current_revision = connection
                .query_row(
                    "SELECT revision FROM threads WHERE user_id = ?1 AND id = ?2",
                    rusqlite::params![user_id, query.note_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "THREAD_NOT_FOUND".to_string())?;
            let mut statement = connection
                .prepare(
                    "SELECT * FROM thread_revisions
                     WHERE user_id = ?1 AND thread_id = ?2 AND (?3 IS NULL OR seq < ?3)
                     ORDER BY seq DESC LIMIT ?4",
                )
                .map_err(|e| e.to_string())?;
            let revisions = statement
                .query_map(
                    rusqlite::params![user_id, query.note_id, query.before_seq, limit],
                    |row| {
                        Ok(V2HistoryRevision {
                            revision: row.get("revision")?,
                            deleted: row.get::<_, i64>("deleted")? != 0,
                            relative_path: row.get("title")?,
                            content_hash: row.get("content_hash")?,
                            size_bytes: row.get("size_bytes")?,
                            sync_seq: row.get("seq")?,
                            created_at: row.get("created_at")?,
                        })
                    },
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let next_before_seq = (revisions.len() as i64 == limit)
                .then(|| revisions.last().map(|revision| revision.sync_seq))
                .flatten();
            return Ok::<_, String>(V2History {
                note_id: query.note_id.clone(),
                notebook_id: String::new(),
                current_revision,
                revisions,
                next_before_seq,
            });
        }
        let (notebook_id, current_revision) = connection
            .query_row(
                "SELECT notebook_id, revision FROM notes WHERE user_id = ?1 AND id = ?2",
                rusqlite::params![user_id, query.note_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "NOTE_NOT_FOUND".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT * FROM note_revisions
                 WHERE user_id = ?1 AND note_id = ?2 AND (?3 IS NULL OR seq < ?3)
                 ORDER BY seq DESC LIMIT ?4",
            )
            .map_err(|e| e.to_string())?;
        let revisions = statement
            .query_map(
                rusqlite::params![user_id, query.note_id, query.before_seq, limit],
                |row| {
                    Ok(V2HistoryRevision {
                        revision: row.get("revision")?,
                        deleted: row.get::<_, i64>("deleted")? != 0,
                        relative_path: row.get("relative_path")?,
                        content_hash: row.get("content_hash")?,
                        size_bytes: row.get("size_bytes")?,
                        sync_seq: row.get("seq")?,
                        created_at: row.get("created_at")?,
                    })
                },
            )
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let next_before_seq = (revisions.len() as i64 == limit)
            .then(|| revisions.last().map(|revision| revision.sync_seq))
            .flatten();
        Ok::<_, String>(V2History {
            note_id: query.note_id.clone(),
            notebook_id,
            current_revision,
            revisions,
            next_before_seq,
        })
    });
    match data {
        Ok(data) => data_reply(data),
        Err(message) if message == "NOTE_NOT_FOUND" => {
            api_error(StatusCode::NOT_FOUND, "NOTE_NOT_FOUND", "笔记不存在")
        }
        Err(message) if message == "THREAD_NOT_FOUND" => {
            api_error(StatusCode::NOT_FOUND, "THREAD_NOT_FOUND", "线程不存在")
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushBody {
    pub protocol_epoch: i64,
    pub operations: Vec<V2PushOperation>,
}

pub async fn push(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<PushBody>,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if body.protocol_epoch != EPOCH {
        return api_error(StatusCode::BAD_REQUEST, "PROTOCOL_MISMATCH", "仅支持协议 epoch 2");
    }
    let result = state.db.with_transaction(|connection| {
        let mut results = Vec::with_capacity(body.operations.len());
        for operation in &body.operations {
            results.push(apply_operation(connection, &user_id, operation));
        }
        let head_cursor = user_head(connection, &user_id);
        Ok::<_, String>(V2PushResult {
            protocol_epoch: EPOCH,
            results,
            head_cursor,
        })
    });
    match result {
        Ok(data) => data_reply(data),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

// ── 逐操作应用 ─────────────────────────────────────────────

fn ok_result(operation_id: &str, data: V2OperationData) -> V2OperationResult {
    V2OperationResult {
        operation_id: operation_id.to_string(),
        ok: true,
        status: 200,
        data: Some(data),
        error: None,
    }
}

fn conflict_result(operation_id: &str, current_revision: &str) -> V2OperationResult {
    V2OperationResult {
        operation_id: operation_id.to_string(),
        ok: false,
        status: 409,
        data: None,
        error: Some(V2OperationError {
            code: "REVISION_CONFLICT".to_string(),
            message: "远端已有更新版本，请同步后再编辑".to_string(),
            details: Some(serde_json::json!({ "currentRevision": current_revision })),
        }),
    }
}

fn entity_error(
    operation_id: &str,
    status: u16,
    code: &str,
    message: &str,
) -> V2OperationResult {
    V2OperationResult {
        operation_id: operation_id.to_string(),
        ok: false,
        status,
        data: None,
        error: Some(V2OperationError {
            code: code.to_string(),
            message: message.to_string(),
            details: None,
        }),
    }
}

/// base 匹配检查：null base 视为「新建」（已存在 → 冲突）。
fn base_matches(current: Option<&str>, base_revision: Option<&str>) -> bool {
    match (current, base_revision) {
        (None, None) => true,
        (Some(_), None) => false,
        (None, Some(_)) => false,
        (Some(current), Some(base)) => current == base,
    }
}

fn apply_operation(
    connection: &Connection,
    user_id: &str,
    operation: &V2PushOperation,
) -> V2OperationResult {
    match operation {
        V2PushOperation::NotebookPut {
            operation_id,
            base_revision,
            notebook,
        } => apply_notebook_put(connection, user_id, operation_id, base_revision.as_deref(), notebook),
        V2PushOperation::NotebookDelete {
            operation_id,
            base_revision,
            notebook_id,
            ..
        } => apply_notebook_delete(connection, user_id, operation_id, base_revision.as_deref(), notebook_id),
        V2PushOperation::NotePut {
            operation_id,
            base_revision,
            note,
        } => apply_note_put(connection, user_id, operation_id, base_revision.as_deref(), note),
        V2PushOperation::NoteDelete {
            operation_id,
            base_revision,
            note_id,
        } => apply_note_delete(connection, user_id, operation_id, base_revision.as_deref(), note_id),
        V2PushOperation::NoteMove {
            operation_id,
            base_revision,
            notebook_id,
            from_note_id,
            from_path: _,
            to_note_id,
            to_path,
        } => apply_note_move(connection, user_id, operation_id, base_revision.as_deref(), notebook_id, from_note_id, to_note_id, to_path),
        V2PushOperation::ThreadPut {
            operation_id,
            base_revision,
            thread,
        } => apply_thread_put(connection, user_id, operation_id, base_revision.as_deref(), thread),
        V2PushOperation::ThreadDelete {
            operation_id,
            base_revision,
            thread_id,
        } => apply_thread_delete(connection, user_id, operation_id, base_revision.as_deref(), thread_id),
    }
}

struct EntitySnapshot {
    revision: String,
}

fn notebook_snapshot(connection: &Connection, user_id: &str, notebook_id: &str) -> Option<EntitySnapshot> {
    connection
        .query_row(
            "SELECT revision FROM notebooks WHERE user_id = ?1 AND id = ?2",
            rusqlite::params![user_id, notebook_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|revision| EntitySnapshot { revision })
}

fn apply_notebook_put(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    notebook: &flowix_sync::V2NotebookPut,
) -> V2OperationResult {
    let snapshot = notebook_snapshot(connection, user_id, &notebook.id);
    // create-or-adopt：null base + 已存在 → 接受（见模块注释 §3）
    let base_ok = match (&snapshot, base_revision) {
        (None, None) => true,
        (Some(_), None) => true,
        (Some(snapshot), Some(base)) => snapshot.revision == base,
        (None, Some(_)) => false,
    };
    if !base_ok {
        return conflict_result(
            operation_id,
            &snapshot.as_ref().map(|value| value.revision.clone()).unwrap_or_default(),
        );
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let attachments = serde_json::to_string(&Vec::<V2Attachment>::new()).unwrap();
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "notebook",
            entity_id: notebook.id.clone(),
            notebook_id: None,
            revision: revision.clone(),
            kind: "put",
            name: Some(notebook.name.clone()),
            icon: notebook.icon.clone(),
            sort_order: Some(notebook.sort_order),
            filename: None,
            agent_id: None,
            content_hash: None,
            size_bytes: None,
            deleted: false,
            attachments,
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    let replaced = snapshot.as_ref().map(|value| value.revision.clone());
    let write = if snapshot.is_some() {
        connection.execute(
            "UPDATE notebooks SET name=?1, icon=?2, sort_order=?3, revision=?4, deleted=0, sync_seq=?5, updated_at=?6
             WHERE user_id=?7 AND id=?8",
            rusqlite::params![
                notebook.name, notebook.icon, notebook.sort_order, revision, sync_seq, now,
                user_id, notebook.id
            ],
        )
    } else {
        connection.execute(
            "INSERT INTO notebooks (user_id, id, name, icon, sort_order, revision, deleted, sync_seq, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8,?8)",
            rusqlite::params![
                user_id, notebook.id, notebook.name, notebook.icon, notebook.sort_order,
                revision, sync_seq, now
            ],
        )
    };
    if let Err(error) = write {
        return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
    }
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "notebook".into(),
            entity_id: notebook.id.clone(),
            revision,
            sync_seq,
            deleted: false,
            resolution: if replaced.is_some() { "updated" } else { "created" }.into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: replaced,
            content_hash: None,
            source_revision: None,
            source_sync_seq: None,
        },
    )
}

struct NoteRowSnapshot {
    notebook_id: String,
    filename: String,
    revision: String,
    content_hash: Option<String>,
    size_bytes: i64,
    attachments: String,
    deleted: bool,
}

fn note_row(connection: &Connection, user_id: &str, note_id: &str) -> Option<NoteRowSnapshot> {
    connection
        .query_row(
            "SELECT notebook_id, filename, revision, content_hash, size_bytes, attachments, deleted
             FROM notes WHERE user_id = ?1 AND id = ?2",
            rusqlite::params![user_id, note_id],
            |row| {
                Ok(NoteRowSnapshot {
                    notebook_id: row.get(0)?,
                    filename: row.get(1)?,
                    revision: row.get(2)?,
                    content_hash: row.get(3)?,
                    size_bytes: row.get(4)?,
                    attachments: row.get(5)?,
                    deleted: row.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
}

fn apply_note_put(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    note: &flowix_sync::V2NotePut,
) -> V2OperationResult {
    let snapshot = note_row(connection, user_id, &note.id);
    // 笔记内容走严格检查（三方合并的前提）
    if !base_matches(snapshot.as_ref().map(|value| value.revision.as_str()), base_revision) {
        return conflict_result(
            operation_id,
            &snapshot.map(|value| value.revision).unwrap_or_default(),
        );
    }
    let blob_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM blobs WHERE content_hash = ?1)",
            [&note.content_hash],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !blob_exists {
        return entity_error(
            operation_id,
            400,
            "BLOB_MISSING",
            &format!("内容 {} 未上传", &note.content_hash[..note.content_hash.len().min(8)]),
        );
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let attachments = serde_json::to_string(&note.attachments).unwrap_or_else(|_| "[]".into());
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "note",
            entity_id: note.id.clone(),
            notebook_id: Some(note.notebook_id.clone()),
            revision: revision.clone(),
            kind: "put",
            name: None,
            icon: None,
            sort_order: None,
            filename: Some(note.filename.clone()),
            agent_id: None,
            content_hash: Some(note.content_hash.clone()),
            size_bytes: Some(note.size_bytes),
            deleted: false,
            attachments: attachments.clone(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    let replaced = snapshot.as_ref().map(|value| value.revision.clone());
    let write = if snapshot.is_some() {
        connection.execute(
            "UPDATE notes SET notebook_id=?1, filename=?2, revision=?3, content_hash=?4,
                    size_bytes=?5, deleted=0, attachments=?6, moved_from_note_id=NULL, sync_seq=?7, updated_at=?8
             WHERE user_id=?9 AND id=?10",
            rusqlite::params![
                note.notebook_id, note.filename, revision, note.content_hash, note.size_bytes,
                attachments, sync_seq, now, user_id, note.id
            ],
        )
    } else {
        connection.execute(
            "INSERT INTO notes (user_id, id, notebook_id, filename, revision, content_hash,
                               size_bytes, deleted, attachments, sync_seq, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,0,?8,?9,?10,?10)",
            rusqlite::params![
                user_id, note.id, note.notebook_id, note.filename, revision, note.content_hash,
                note.size_bytes, attachments, sync_seq, now
            ],
        )
    };
    if let Err(error) = write {
        return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
    }
    if let Err(error) = connection.execute(
        "INSERT INTO note_revisions (user_id, note_id, notebook_id, revision, deleted, relative_path, content_hash, size_bytes, created_at)
         VALUES (?1,?2,?3,?4,0,?5,?6,?7,?8)",
        rusqlite::params![
            user_id, note.id, note.notebook_id, revision, note.filename, note.content_hash,
            note.size_bytes, now
        ],
    ) {
        return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
    }
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "note".into(),
            entity_id: note.id.clone(),
            revision,
            sync_seq,
            deleted: false,
            resolution: if replaced.is_some() { "updated" } else { "created" }.into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: replaced,
            content_hash: Some(note.content_hash.clone()),
            source_revision: None,
            source_sync_seq: None,
        },
    )
}

fn apply_notebook_delete(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    notebook_id: &str,
) -> V2OperationResult {
    let Some(snapshot) = notebook_snapshot(connection, user_id, notebook_id) else {
        return entity_error(operation_id, 404, "NOTEBOOK_NOT_FOUND", "笔记本不存在");
    };
    if !base_matches(Some(snapshot.revision.as_str()), base_revision) {
        return conflict_result(operation_id, &snapshot.revision);
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "notebook",
            entity_id: notebook_id.to_string(),
            notebook_id: None,
            revision: revision.clone(),
            kind: "delete",
            name: None,
            icon: None,
            sort_order: None,
            filename: None,
            agent_id: None,
            content_hash: None,
            size_bytes: None,
            deleted: true,
            attachments: "[]".into(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    if let Err(error) = connection.execute(
        "UPDATE notebooks SET deleted=1, revision=?1, sync_seq=?2, updated_at=?3 WHERE user_id=?4 AND id=?5",
        rusqlite::params![revision, sync_seq, now, user_id, notebook_id],
    ) {
        return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
    }
    let _ = connection.execute(
        "UPDATE notes SET deleted=1, revision=?1, updated_at=?2 WHERE user_id=?3 AND notebook_id=?4 AND deleted=0",
        rusqlite::params![revision, now, user_id, notebook_id],
    );
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "notebook".into(),
            entity_id: notebook_id.to_string(),
            revision,
            sync_seq,
            deleted: true,
            resolution: "deleted".into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: None,
            content_hash: None,
            source_revision: None,
            source_sync_seq: None,
        },
    )
}

fn apply_note_delete(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    note_id: &str,
) -> V2OperationResult {
    let Some(snapshot) = note_row(connection, user_id, note_id) else {
        return entity_error(operation_id, 404, "FILE_NOT_FOUND", "笔记不存在");
    };
    if !base_matches(Some(snapshot.revision.as_str()), base_revision) {
        return conflict_result(operation_id, &snapshot.revision);
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "note",
            entity_id: note_id.to_string(),
            notebook_id: Some(snapshot.notebook_id.clone()),
            revision: revision.clone(),
            kind: "delete",
            name: None,
            icon: None,
            sort_order: None,
            filename: Some(snapshot.filename.clone()),
            agent_id: None,
            content_hash: None,
            size_bytes: None,
            deleted: true,
            attachments: "[]".into(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    let edits = [
        connection.execute(
            "UPDATE notes SET deleted=1, revision=?1, sync_seq=?2, updated_at=?3 WHERE user_id=?4 AND id=?5",
            rusqlite::params![revision, sync_seq, now, user_id, note_id],
        ),
        connection.execute(
            "INSERT INTO note_revisions (user_id, note_id, notebook_id, revision, deleted, relative_path, content_hash, size_bytes, created_at)
             VALUES (?1,?2,?3,?4,1,?5,NULL,0,?6)",
            rusqlite::params![user_id, note_id, snapshot.notebook_id, revision, snapshot.filename, now],
        ),
    ];
    for edit in edits {
        if let Err(error) = edit {
            return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
        }
    }
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "note".into(),
            entity_id: note_id.to_string(),
            revision,
            sync_seq,
            deleted: true,
            resolution: "deleted".into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: None,
            content_hash: None,
            source_revision: None,
            source_sync_seq: None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_note_move(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    notebook_id: &str,
    from_note_id: &str,
    to_note_id: &str,
    to_path: &str,
) -> V2OperationResult {
    let Some(from) = note_row(connection, user_id, from_note_id) else {
        return entity_error(operation_id, 404, "FILE_NOT_FOUND", "源笔记不存在");
    };
    if from.deleted {
        return entity_error(operation_id, 404, "FILE_NOT_FOUND", "源笔记不存在");
    }
    if !base_matches(Some(from.revision.as_str()), base_revision) {
        return conflict_result(operation_id, &from.revision);
    }
    if let Some(target) = note_row(connection, user_id, to_note_id) {
        if !target.deleted {
            return entity_error(operation_id, 409, "PATH_CONFLICT", "目标路径已被占用");
        }
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "note",
            entity_id: to_note_id.to_string(),
            notebook_id: Some(notebook_id.to_string()),
            revision: revision.clone(),
            kind: "put",
            name: None,
            icon: None,
            sort_order: None,
            filename: Some(to_path.to_string()),
            agent_id: None,
            content_hash: from.content_hash.clone(),
            size_bytes: Some(from.size_bytes),
            deleted: false,
            attachments: from.attachments.clone(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    let source_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "note",
            entity_id: from_note_id.to_string(),
            notebook_id: Some(from.notebook_id.clone()),
            revision: revision.clone(),
            kind: "delete",
            name: None,
            icon: None,
            sort_order: None,
            filename: Some(from.filename.clone()),
            agent_id: None,
            content_hash: None,
            size_bytes: None,
            deleted: true,
            attachments: "[]".into(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    // 移动 = 旧 note 标删 + 新 note（movedFromNoteId 指向来源）
    let edits = [
        connection.execute(
            "UPDATE notes SET deleted=1, revision=?1, sync_seq=?2, updated_at=?3 WHERE user_id=?4 AND id=?5",
            rusqlite::params![revision, source_seq, now, user_id, from_note_id],
        ),
        connection.execute(
            "INSERT INTO notes (user_id, id, notebook_id, filename, moved_from_note_id, revision,
                               content_hash, size_bytes, deleted, attachments, sync_seq, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,0,?9,?10,?11,?11)
             ON CONFLICT(user_id, id) DO UPDATE SET
               deleted=0, filename=excluded.filename, notebook_id=excluded.notebook_id,
               moved_from_note_id=excluded.moved_from_note_id, revision=excluded.revision,
               content_hash=excluded.content_hash, size_bytes=excluded.size_bytes,
               attachments=excluded.attachments, sync_seq=excluded.sync_seq,
               updated_at=excluded.updated_at",
            rusqlite::params![
                user_id, to_note_id, notebook_id, to_path, from_note_id, revision,
                from.content_hash, from.size_bytes, from.attachments, sync_seq, now
            ],
        ),
        connection.execute(
            "INSERT INTO note_revisions (user_id, note_id, notebook_id, revision, deleted, relative_path, content_hash, size_bytes, created_at)
             VALUES (?1,?2,?3,?4,1,?5,NULL,0,?6)",
            rusqlite::params![user_id, from_note_id, from.notebook_id, revision, from.filename, now],
        ),
        connection.execute(
            "INSERT INTO note_revisions (user_id, note_id, notebook_id, revision, deleted, relative_path, content_hash, size_bytes, created_at)
             VALUES (?1,?2,?3,?4,0,?5,?6,?7,?8)",
            rusqlite::params![
                user_id, to_note_id, notebook_id, revision, to_path, from.content_hash,
                from.size_bytes, now
            ],
        ),
    ];
    for edit in edits {
        if let Err(error) = edit {
            return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
        }
    }
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "note".into(),
            entity_id: to_note_id.to_string(),
            revision,
            sync_seq,
            deleted: false,
            resolution: "moved".into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: None,
            content_hash: from.content_hash,
            source_revision: None,
            source_sync_seq: Some(source_seq),
        },
    )
}

// ── thread 实体（方案 A：整 Thread 单 JSON blob）─────────────

struct ThreadRowSnapshot {
    agent_id: String,
    title: String,
    revision: String,
}

fn thread_row(connection: &Connection, user_id: &str, thread_id: &str) -> Option<ThreadRowSnapshot> {
    connection
        .query_row(
            "SELECT agent_id, title, revision FROM threads WHERE user_id = ?1 AND id = ?2",
            rusqlite::params![user_id, thread_id],
            |row| {
                Ok(ThreadRowSnapshot {
                    agent_id: row.get(0)?,
                    title: row.get(1)?,
                    revision: row.get(2)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
}

/// thread.put 与 note 同款严格 base 检查：并发冲突靠客户端做
/// 消息级合并（本地 + 云端 blob 内容按消息 id 并集）后以
/// currentRevision 为 base 重推，服务端不做自动合并。
fn apply_thread_put(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    thread: &flowix_sync::V2ThreadPut,
) -> V2OperationResult {
    let snapshot = thread_row(connection, user_id, &thread.id);
    if !base_matches(
        snapshot.as_ref().map(|value| value.revision.as_str()),
        base_revision,
    ) {
        return conflict_result(
            operation_id,
            &snapshot.map(|value| value.revision).unwrap_or_default(),
        );
    }
    let blob_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM blobs WHERE content_hash = ?1)",
            [&thread.content_hash],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !blob_exists {
        return entity_error(
            operation_id,
            400,
            "BLOB_MISSING",
            &format!("内容 {} 未上传", &thread.content_hash[..thread.content_hash.len().min(8)]),
        );
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "thread",
            entity_id: thread.id.clone(),
            notebook_id: None,
            revision: revision.clone(),
            kind: "put",
            name: Some(thread.title.clone()),
            icon: None,
            sort_order: None,
            filename: None,
            agent_id: Some(thread.agent_id.clone()),
            content_hash: Some(thread.content_hash.clone()),
            size_bytes: Some(thread.size_bytes),
            deleted: false,
            attachments: "[]".into(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    let replaced = snapshot.as_ref().map(|value| value.revision.clone());
    let write = if snapshot.is_some() {
        connection.execute(
            "UPDATE threads SET agent_id=?1, title=?2, revision=?3, content_hash=?4,
                    size_bytes=?5, deleted=0, sync_seq=?6, updated_at=?7
             WHERE user_id=?8 AND id=?9",
            rusqlite::params![
                thread.agent_id, thread.title, revision, thread.content_hash, thread.size_bytes,
                sync_seq, now, user_id, thread.id
            ],
        )
    } else {
        connection.execute(
            "INSERT INTO threads (user_id, id, agent_id, title, revision, content_hash,
                                  size_bytes, deleted, sync_seq, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,0,?8,?9,?9)",
            rusqlite::params![
                user_id, thread.id, thread.agent_id, thread.title, revision, thread.content_hash,
                thread.size_bytes, sync_seq, now
            ],
        )
    };
    if let Err(error) = write {
        return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
    }
    if let Err(error) = connection.execute(
        "INSERT INTO thread_revisions (user_id, thread_id, revision, deleted, title, content_hash, size_bytes, created_at)
         VALUES (?1,?2,?3,0,?4,?5,?6,?7)",
        rusqlite::params![
            user_id, thread.id, revision, thread.title, thread.content_hash, thread.size_bytes, now
        ],
    ) {
        return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
    }
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "thread".into(),
            entity_id: thread.id.clone(),
            revision,
            sync_seq,
            deleted: false,
            resolution: if replaced.is_some() { "updated" } else { "created" }.into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: replaced,
            content_hash: Some(thread.content_hash.clone()),
            source_revision: None,
            source_sync_seq: None,
        },
    )
}

fn apply_thread_delete(
    connection: &Connection,
    user_id: &str,
    operation_id: &str,
    base_revision: Option<&str>,
    thread_id: &str,
) -> V2OperationResult {
    let Some(snapshot) = thread_row(connection, user_id, thread_id) else {
        return entity_error(operation_id, 404, "THREAD_NOT_FOUND", "线程不存在");
    };
    if !base_matches(Some(snapshot.revision.as_str()), base_revision) {
        return conflict_result(operation_id, &snapshot.revision);
    }
    let revision = crate::db::new_revision();
    let now = crate::now_ms();
    let sync_seq = match record_change(
        connection,
        user_id,
        ChangeRow {
            entity_type: "thread",
            entity_id: thread_id.to_string(),
            notebook_id: None,
            revision: revision.clone(),
            kind: "delete",
            name: Some(snapshot.title.clone()),
            icon: None,
            sort_order: None,
            filename: None,
            agent_id: Some(snapshot.agent_id.clone()),
            content_hash: None,
            size_bytes: None,
            deleted: true,
            attachments: "[]".into(),
        },
    ) {
        Ok(value) => value,
        Err(error) => return entity_error(operation_id, 500, "INTERNAL_ERROR", &error),
    };
    let edits = [
        connection.execute(
            "UPDATE threads SET deleted=1, revision=?1, sync_seq=?2, updated_at=?3 WHERE user_id=?4 AND id=?5",
            rusqlite::params![revision, sync_seq, now, user_id, thread_id],
        ),
        connection.execute(
            "INSERT INTO thread_revisions (user_id, thread_id, revision, deleted, title, content_hash, size_bytes, created_at)
             VALUES (?1,?2,?3,1,?4,NULL,0,?5)",
            rusqlite::params![user_id, thread_id, revision, snapshot.title, now],
        ),
    ];
    for edit in edits {
        if let Err(error) = edit {
            return entity_error(operation_id, 500, "INTERNAL_ERROR", &error.to_string());
        }
    }
    ok_result(
        operation_id,
        V2OperationData {
            entity_type: "thread".into(),
            entity_id: thread_id.to_string(),
            revision,
            sync_seq,
            deleted: true,
            resolution: "deleted".into(),
            base_revision: base_revision.map(str::to_string),
            replaced_revision: None,
            content_hash: None,
            source_revision: None,
            source_sync_seq: None,
        },
    )
}

