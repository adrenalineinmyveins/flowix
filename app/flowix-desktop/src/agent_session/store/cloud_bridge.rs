//! 方案 A3 云同步桥：thread.db 与 `V2ThreadPayload` 的双向投影。
//!
//! - 出向：`thread_payload_for_cloud` 把 threads + thread_messages 投影成
//!   整线程 JSON（消息按 (createdAt,id) 排序保证序列化确定性，使内容
//!   哈希可作同步 fingerprint；tool_data 超限截断，R7 隐私/体积防线）。
//! - 入向：`upsert_thread_from_cloud` 幂等落库（消息按 id INSERT OR
//!   IGNORE，新消息按序分配 sequence），与引擎 409 消解的两段式约定
//!   （"桥接层 upsert 后 re-dirty"）配套。
//!
//! 只投影产品线程（消息在 thread_messages 表）；外部 runtime 的消息在
//! agent_external_events（C7 排除面），不参与云同步。

use super::ThreadManager;
use crate::agent_session::error::ThreadError;
use flowix_sync::{V2ThreadMessage, V2ThreadPayload};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

/// 云侧 tool_data 单值上限（R7）：超限截断并加标记，避免超大工具
/// 输出把每回合全量重传放大到 MB 级。
const CLOUD_TOOL_DATA_LIMIT_BYTES: usize = 64 * 1024;
const CLOUD_TOOL_DATA_TRUNCATED_SUFFIX: &str = "\n[truncated for cloud sync]";

/// 消息 timestamp（TEXT，前端多为 ISO/RFC3339，历史数据可能为毫秒数字
/// 字符串）→ 毫秒。解析失败回退 0（排序退化为 id 序，不炸同步）。
pub(crate) fn message_timestamp_to_millis(timestamp: &str) -> i64 {
    let trimmed = timestamp.trim();
    if trimmed.is_empty() {
        return 0;
    }
    if let Ok(millis) = trimmed.parse::<i64>() {
        return millis;
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return parsed.timestamp_millis();
    }
    chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
        .map(|parsed| parsed.and_utc().timestamp_millis())
        .unwrap_or(0)
}

fn millis_to_timestamp(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| millis.to_string())
}

fn truncate_tool_data(value: Option<String>) -> Option<String> {
    value.map(|data| {
        if data.len() > CLOUD_TOOL_DATA_LIMIT_BYTES {
            // 按字符边界粗截断（UTF-8 安全）：先切字节再确认边界。
            let mut cut = CLOUD_TOOL_DATA_LIMIT_BYTES;
            while cut > 0 && !data.is_char_boundary(cut) {
                cut -= 1;
            }
            format!("{}{}", &data[..cut], CLOUD_TOOL_DATA_TRUNCATED_SUFFIX)
        } else {
            data
        }
    })
}

struct ThreadHead {
    agent_id: String,
    title: String,
    created_at: i64,
}

impl ThreadManager {
    /// 云实体 id ↔ 本地 thread_id 映射（本地零迁移，云端用 `th_` 前缀
    /// 隔离 id 空间，R8：history 端点按前缀路由）。
    pub(crate) fn cloud_thread_id(local_thread_id: &str) -> String {
        format!("th_{local_thread_id}")
    }

    pub(crate) fn local_thread_id(cloud_thread_id: &str) -> Option<&str> {
        cloud_thread_id.strip_prefix("th_")
    }

    /// `updated_at > cutoff` 且有消息的线程（对账增量扫描；cutoff=0 全量）。
    pub(crate) async fn thread_ids_with_messages_since(
        self: &Arc<Self>,
        cutoff_ms: i64,
    ) -> Result<Vec<(String, i64)>, ThreadError> {
        self.run_blocking(move |tm| {
            let conn = tm.lock_conn();
            let mut stmt = conn.prepare(
                "SELECT t.thread_id, t.updated_at FROM threads t
                 WHERE t.updated_at > ?1
                   AND EXISTS(SELECT 1 FROM thread_messages m WHERE m.thread_id = t.thread_id)
                 ORDER BY t.updated_at",
            )?;
            let rows = stmt
                .query_map([cutoff_ms], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    /// 全部有消息的线程 id（删除对账基准）。
    pub(crate) async fn all_thread_ids_with_messages(self: &Arc<Self>) -> Result<Vec<String>, ThreadError> {
        self.run_blocking(move |tm| {
            let conn = tm.lock_conn();
            let mut stmt = conn.prepare(
                "SELECT t.thread_id FROM threads t
                 WHERE EXISTS(SELECT 1 FROM thread_messages m WHERE m.thread_id = t.thread_id)",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    /// 出向投影：本地线程 → 云 payload。消息按 (createdAt, id) 排序，
    /// 序列化确定 → `v2_content_hash(payload)` 可作同步 fingerprint。
    pub(crate) async fn thread_payload_for_cloud(
        self: &Arc<Self>,
        local_thread_id: &str,
    ) -> Result<Option<V2ThreadPayload>, ThreadError> {
        let local_thread_id = local_thread_id.to_string();
        self.run_blocking(move |tm| {
            let conn = tm.lock_conn();
            let Some(head) = read_thread_head(&conn, &local_thread_id)? else {
                return Ok(None);
            };
            let mut stmt = conn.prepare(
                "SELECT id, role, content, tool_name, tool_data, tool_input, reasoning, timestamp
                 FROM thread_messages WHERE thread_id = ?1 ORDER BY sequence",
            )?;
            let rows = stmt
                .query_map([&local_thread_id], |row| {
                    Ok(V2ThreadMessage {
                        id: row.get(0)?,
                        role: row.get(1)?,
                        content: row.get(2)?,
                        reasoning: row.get(6)?,
                        tool_name: row.get(3)?,
                        tool_input: row.get(5)?,
                        tool_data: truncate_tool_data(row.get(4)?),
                        created_at: message_timestamp_to_millis(&row.get::<_, String>(7)?),
                        origin: Some("desktop".into()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut messages = rows;
            messages.sort_by(|left, right| {
                (left.created_at, &left.id).cmp(&(right.created_at, &right.id))
            });
            Ok(Some(V2ThreadPayload {
                version: 1,
                thread_id: Self::cloud_thread_id(&local_thread_id),
                agent_id: head.agent_id,
                title: head.title,
                created_at: head.created_at,
                messages,
            }))
        })
        .await
    }

    /// 入向 upsert：云 payload → 本地线程。消息按 id 幂等（已存在跳过），
    /// 新消息按 payload 序分配 sequence；title 云端头为准；返回是否有变化
    ///（新线程或新消息），供调用方决定是否发 UI 事件。
    pub(crate) async fn upsert_thread_from_cloud(
        self: &Arc<Self>,
        payload: &V2ThreadPayload,
    ) -> Result<bool, ThreadError> {
        let Some(local_id) = Self::local_thread_id(&payload.thread_id).map(str::to_owned) else {
            return Err(ThreadError::NotFound(format!(
                "cloud thread id {} lacks th_ prefix",
                payload.thread_id
            )));
        };
        let payload = payload.clone();
        self.run_blocking(move |tm| {
            let now = chrono::Utc::now().timestamp_millis();
            let mut conn = tm.lock_conn();
            let tx = conn.transaction()?;
            let existing = read_thread_head(&tx, &local_id)?;
            let mut changed = existing.is_none();
            if let Some(head) = &existing {
                if head.title != payload.title {
                    tx.execute(
                        "UPDATE threads SET title = ?2, updated_at = ?3 WHERE thread_id = ?1",
                        params![local_id, payload.title, now],
                    )?;
                    changed = true;
                }
            } else {
                tx.execute(
                    "INSERT INTO threads (thread_id, agent_id, title, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        local_id,
                        payload.agent_id,
                        payload.title,
                        payload.created_at.max(0),
                        now
                    ],
                )?;
                tm.ensure_simplified_thread_index(&tx, &local_id)?;
            }
            let mut sequence: i64 = tx.query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM thread_messages WHERE thread_id = ?1",
                [&local_id],
                |row| row.get(0),
            )?;
            for message in &payload.messages {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM thread_messages WHERE id = ?1)",
                        [&message.id],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if exists {
                    continue;
                }
                sequence += 1;
                tx.execute(
                    "INSERT INTO thread_messages (
                        id, thread_id, role, content, timestamp,
                        tool_name, tool_data, tool_input, reasoning, sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        message.id,
                        local_id,
                        message.role,
                        message.content,
                        millis_to_timestamp(message.created_at),
                        message.tool_name,
                        message.tool_data,
                        message.tool_input,
                        message.reasoning,
                        sequence,
                    ],
                )?;
                changed = true;
            }
            if changed {
                tx.execute(
                    "UPDATE threads SET updated_at = ?2 WHERE thread_id = ?1",
                    params![local_id, now],
                )?;
            }
            tx.commit()?;
            Ok(changed)
        })
        .await
    }
}

fn read_thread_head(conn: &Connection, local_thread_id: &str) -> Result<Option<ThreadHead>, ThreadError> {
    Ok(conn
        .query_row(
            "SELECT agent_id, title, created_at FROM threads WHERE thread_id = ?1",
            [local_thread_id],
            |row| {
                Ok(ThreadHead {
                    agent_id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_session::types::ChatMessage;
    use crate::agent_types::AgentId;

    fn message(id: &str, role: &str, content: &str, timestamp: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role: role.into(),
            message_type: None,
            content: content.into(),
            attachments: None,
            llm_content: None,
            system_reminder_directory: None,
            timestamp: timestamp.into(),
            source_sequence: None,
            is_loading: None,
            tool_call_id: None,
            tool_name: None,
            tool_data: None,
            tool_input: None,
            tool_call: None,
            tool_result: None,
            tool_calls: None,
            reasoning: None,
            is_completed: None,
            error_details: None,
            is_collapsed: None,
            codex_turn_id: None,
            turn_duration_ms: None,
        }
    }

    #[tokio::test]
    async fn payload_projection_roundtrips_messages_with_stable_hash() {
        let manager = ThreadManager::for_tests();
        let thread = manager
            .create_thread(AgentId("default".into()), "桥接测试".into())
            .await
            .unwrap();
        manager
            .add_message(
                &thread.thread_id,
                message("msg_1", "user", "你好", "2026-10-19T10:00:00+08:00"),
            )
            .await
            .unwrap();
        manager
            .add_message(
                &thread.thread_id,
                message(
                    "msg_2",
                    "assistant",
                    "在的",
                    "1760834400000", // 毫秒数字字符串（历史格式兼容）
                ),
            )
            .await
            .unwrap();

        let payload = manager
            .thread_payload_for_cloud(&thread.thread_id)
            .await
            .unwrap()
            .expect("payload exists");
        assert_eq!(
            payload.thread_id,
            ThreadManager::cloud_thread_id(&thread.thread_id)
        );
        assert!(payload.thread_id.starts_with("th_thread_"));
        assert_eq!(payload.title, "桥接测试");
        assert_eq!(payload.messages.len(), 2);
        // (createdAt,id) 排序：msg_2（2025 毫秒串）早于 msg_1（2026 ISO）→ msg_2 在前
        assert_eq!(payload.messages[0].id, "msg_2");
        assert_eq!(payload.messages[0].created_at, 1760834400000, "毫秒字符串直通");
        assert!(payload.messages[1].created_at > 0, "ISO timestamp 解析为毫秒");

        // 确定性：重复投影哈希一致（fingerprint 语义的前提）
        let again = manager
            .thread_payload_for_cloud(&thread.thread_id)
            .await
            .unwrap()
            .unwrap();
        let first = serde_json::to_vec(&payload).unwrap();
        let second = serde_json::to_vec(&again).unwrap();
        assert_eq!(flowix_sync::v2_content_hash(&first), flowix_sync::v2_content_hash(&second));
    }

    #[tokio::test]
    async fn upsert_is_idempotent_and_unions_messages() {
        let manager = ThreadManager::for_tests();
        // 云端 payload（模拟另一端）：msg_1（与本地重叠）+ msg_cloud（远端新增）
        let thread = manager
            .create_thread(AgentId("default".into()), "本地".into())
            .await
            .unwrap();
        manager
            .add_message(&thread.thread_id, message("msg_1", "user", "共同", "2026-10-19T10:00:00+08:00"))
            .await
            .unwrap();
        let cloud_payload = V2ThreadPayload {
            version: 1,
            thread_id: ThreadManager::cloud_thread_id(&thread.thread_id),
            agent_id: "default".into(),
            title: "云端改名".into(),
            created_at: 1,
            messages: vec![
                V2ThreadMessage {
                    id: "msg_1".into(),
                    role: "user".into(),
                    content: "共同".into(),
                    reasoning: None,
                    tool_name: None,
                    tool_input: None,
                    tool_data: None,
                    created_at: 100,
                    origin: Some("mobile".into()),
                },
                V2ThreadMessage {
                    id: "msg_cloud".into(),
                    role: "assistant".into(),
                    content: "远端回答".into(),
                    reasoning: None,
                    tool_name: None,
                    tool_input: None,
                    tool_data: None,
                    created_at: 200,
                    origin: Some("mobile".into()),
                },
            ],
        };

        let changed = manager
            .upsert_thread_from_cloud(&cloud_payload)
            .await
            .unwrap();
        assert!(changed, "新消息 + title 变化");
        // 再投影：本地 == 云端内容（title 已同步、消息并集），clean 语义
        let projected = manager
            .thread_payload_for_cloud(&thread.thread_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(projected.title, "云端改名");
        assert_eq!(projected.messages.len(), 2);

        // 幂等：同 payload 再 upsert → 无变化
        let changed_again = manager
            .upsert_thread_from_cloud(&cloud_payload)
            .await
            .unwrap();
        assert!(!changed_again, "重复 apply 无新消息且 title 已一致");

        // 全新线程（移动端新建，桌面无记录）：创建 + 全量消息
        let fresh = V2ThreadPayload {
            version: 1,
            thread_id: "th_thread_mobile_new".into(),
            agent_id: "builtin.chat".into(),
            title: "移动端会话".into(),
            created_at: 42,
            messages: vec![V2ThreadMessage {
                id: "msg_m1".into(),
                role: "user".into(),
                content: "hi".into(),
                reasoning: None,
                tool_name: None,
                tool_input: None,
                tool_data: None,
                created_at: 50,
                origin: Some("mobile".into()),
            }],
        };
        assert!(manager.upsert_thread_from_cloud(&fresh).await.unwrap());
        let projected = manager
            .thread_payload_for_cloud("thread_mobile_new")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(projected.title, "移动端会话");
        assert_eq!(projected.messages.len(), 1);
        assert_eq!(projected.agent_id, "builtin.chat");
    }

    #[test]
    fn tool_data_is_truncated_at_the_cloud_limit() {
        let big = "x".repeat(CLOUD_TOOL_DATA_LIMIT_BYTES + 100);
        let truncated = truncate_tool_data(Some(big)).unwrap();
        assert!(truncated.len() < CLOUD_TOOL_DATA_LIMIT_BYTES + 100);
        assert!(truncated.ends_with(CLOUD_TOOL_DATA_TRUNCATED_SUFFIX.trim()));
        let kept = truncate_tool_data(Some("small".into())).unwrap();
        assert_eq!(kept, "small");
        assert!(truncate_tool_data(None).is_none());
    }

    #[test]
    fn timestamp_parser_covers_iso_millis_and_fallback() {
        assert!(message_timestamp_to_millis("2026-10-19T10:00:00+08:00") > 0);
        assert_eq!(message_timestamp_to_millis("1760834400000"), 1760834400000);
        assert!(message_timestamp_to_millis("2026-10-19 10:00:00") > 0, "无时区格式按本地时间解析");
        assert_eq!(message_timestamp_to_millis(""), 0);
        assert_eq!(message_timestamp_to_millis("not-a-date"), 0, "垃圾输入回退 0");
    }
}
