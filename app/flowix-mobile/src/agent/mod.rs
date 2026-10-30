//! 移动端 AI 对话（方案 A4）：OpenAI 兼容 SSE 直连引擎 + 会话存储 + 云同步桥。
//!
//! - 会话存储：`threads.db` 单表整包 JSON（payload 即 `V2ThreadPayload`，
//!   消息按 id 幂等合并复用 `v2_merge_thread_payloads` 纯函数）；本地与云端
//!   使用同一 `th_` 前缀 id（R8），无映射层。
//! - provider 配置：`agent_provider.json` 平文件（**api_key 绝不上云**，
//!   方案 §8）；SSE 长连接客户端不设总超时。
//! - 写路径收敛在回合结束（stream_end 落库点）：`record_v2_thread_change`
//!   fingerprint 幂等 + 3s 防抖同步，30s 轮询兜底。

mod chat;
pub(crate) mod commands;
pub(crate) mod tools;

use flowix_sync::{V2LocalThread, V2ThreadPayload};
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) const THREADS_CHANGED_EVENT: &str = "threads-changed";

/// OpenAI 兼容 provider 配置（用户自填，私有文件）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderConfig {
    /// API base，如 https://api.deepseek.com（自动拼 /v1/chat/completions）。
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    /// 联网搜索工具（方案 B）：可选 Tavily API key；
    /// 留空时工具走内置必应网页抓取（免 key，质量以能用为先）。
    #[serde(default)]
    pub search_api_key: Option<String>,
}

impl AgentProviderConfig {
    pub fn is_ready(&self) -> bool {
        !self.api_base.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }

    fn chat_completions_url(&self) -> String {
        let base = self.api_base.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            return base.to_string();
        }
        if base.ends_with("/v1") {
            return format!("{base}/chat/completions");
        }
        format!("{base}/v1/chat/completions")
    }
}

pub struct AgentState {
    db_path: PathBuf,
    provider_path: PathBuf,
    conn: Mutex<Connection>,
}

/// 规范化用户输入的 API base：
/// - 去空白与尾斜杠
/// - 裸 scheme（`http:x` / `https:x`，缺 `//`）→ 统一 `https://`
///   （成因多为 https:// 漏字；真 http 自建服务的用户会写全 `http://`）
/// - 无 scheme 的裸域名 → 补 `https://`
fn normalize_api_base(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/').to_string();
    if s.is_empty() {
        return s;
    }
    for scheme in ["https", "http"] {
        if let Some(rest) = s.strip_prefix(&format!("{scheme}://")) {
            return format!("{scheme}://{rest}");
        }
        if let Some(rest) = s.strip_prefix(&format!("{scheme}:")) {
            if !rest.is_empty() && !rest.starts_with('/') {
                return format!("https://{rest}");
            }
        }
    }
    if s.contains("://") {
        return s;
    }
    format!("https://{s}")
}

impl AgentState {
    pub fn new(data_dir: &std::path::Path) -> Result<Self, String> {
        let db_path = data_dir.join("threads.db");
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS threads (
                thread_id TEXT PRIMARY KEY,
                payload_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            db_path,
            provider_path: data_dir.join("agent_provider.json"),
            conn: Mutex::new(conn),
        })
    }

    /// 调试/测试用：库文件路径。
    pub(crate) fn _db_path(&self) -> &std::path::Path {
        &self.db_path
    }

    pub fn provider_config(&self) -> Result<AgentProviderConfig, String> {
        match std::fs::read_to_string(&self.provider_path) {
            Ok(content) => serde_json::from_str(&content)
                .map_err(|e| format!("AGENT_PROVIDER_CONFIG_INVALID: {e}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AgentProviderConfig::default())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn save_provider_config(&self, config: &AgentProviderConfig) -> Result<(), String> {
        // apiBase 规范化：容忍常见笔误（"api.x.com"、缺少斜杠的 "http:api.x.com"），
        // 否则畸形 URL 会让所有对话请求静默失败（用户只看到"没有回复"）。
        let mut config = config.clone();
        config.api_base = normalize_api_base(&config.api_base);
        let content = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        flowix_core::memo_file::atomic_write_bytes(&self.provider_path, content.as_bytes())
            .map_err(|e| e.to_string())
    }

    fn with_conn<T>(&self, run: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "agent store lock poisoned".to_string())?;
        run(&conn)
    }

    pub(crate) fn thread_ids(&self) -> Result<Vec<String>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT thread_id FROM threads ORDER BY updated_at DESC")
                .map_err(|e| e.to_string())?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(ids)
        })
    }

    pub(crate) fn thread_payload(&self, thread_id: &str) -> Result<Option<V2ThreadPayload>, String> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT payload_json FROM threads WHERE thread_id = ?1",
                [thread_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|e| format!("AGENT_THREAD_PAYLOAD_INVALID: {e}"))
        })
    }

    /// 线程 id 归一查询：桌面同步卡存的 threadId 是桌面本地 id（无 `th_`
    /// 前缀），而本地线程库与云端实体恒为 `th_{本地id}`（桌面
    /// cloud_thread_id 映射）。原 id miss 且无前缀时按 `th_{id}` 重试；
    /// 命中返回归一后的 id 与 payload。
    pub(crate) fn thread_payload_normalized(
        &self,
        thread_id: &str,
    ) -> Result<Option<(String, V2ThreadPayload)>, String> {
        if let Some(payload) = self.thread_payload(thread_id)? {
            return Ok(Some((thread_id.to_string(), payload)));
        }
        if !thread_id.starts_with("th_") {
            let normalized = format!("th_{thread_id}");
            if let Some(payload) = self.thread_payload(&normalized)? {
                return Ok(Some((normalized, payload)));
            }
        }
        Ok(None)
    }

    /// 列表行（不反序列化消息体，轻量）。
    pub(crate) fn thread_summaries(&self) -> Result<Vec<ThreadSummary>, String> {
        let ids = self.thread_ids()?;
        let mut summaries = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(payload) = self.thread_payload(&id)? {
                summaries.push(ThreadSummary {
                    thread_id: payload.thread_id.clone(),
                    title: payload.title.clone(),
                    agent_id: payload.agent_id.clone(),
                    updated_at: payload
                        .messages
                        .iter()
                        .map(|message| message.created_at)
                        .max()
                        .unwrap_or(payload.created_at),
                    message_count: payload.messages.len(),
                    starred: payload.starred,
                    archived: payload.archived,
                });
            }
        }
        Ok(summaries)
    }

    /// 更新会话元数据（重命名/收藏/归档；None 字段不动）。返回更新后的 payload。
    pub(crate) fn update_thread_meta(
        &self,
        thread_id: &str,
        title: Option<&str>,
        starred: Option<bool>,
        archived: Option<bool>,
    ) -> Result<Option<V2ThreadPayload>, String> {
        let Some(mut payload) = self.thread_payload(thread_id)? else {
            return Ok(None);
        };
        if let Some(title) = title {
            let trimmed = title.trim();
            if trimmed.is_empty() {
                return Err("AGENT_THREAD_TITLE_EMPTY".into());
            }
            payload.title = trimmed.to_string();
        }
        if let Some(starred) = starred {
            payload.starred = starred;
        }
        if let Some(archived) = archived {
            payload.archived = archived;
        }
        self.save_thread_payload(&payload)?;
        Ok(Some(payload))
    }

    /// 落库（整包替换）。写入方：回合结束、apply 远端。
    pub(crate) fn save_thread_payload(&self, payload: &V2ThreadPayload) -> Result<(), String> {
        let json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().timestamp_millis();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO threads (thread_id, payload_json, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(thread_id) DO UPDATE SET payload_json = excluded.payload_json,
                   updated_at = excluded.updated_at",
                rusqlite::params![payload.thread_id, json, now],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    /// 幂等并集合并（本地为 local、来者为 remote），返回合并后的 payload。
    pub(crate) fn merge_thread_payload(
        &self,
        incoming: &V2ThreadPayload,
    ) -> Result<V2ThreadPayload, String> {
        match self.thread_payload(&incoming.thread_id)? {
            Some(local) => flowix_sync::v2_merge_thread_payloads(&local, incoming)
                .map_err(|e| format!("AGENT_THREAD_MERGE_FAILED: {e}")),
            None => Ok(incoming.clone()),
        }
    }

    pub(crate) fn delete_thread(&self, thread_id: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let removed = conn
                .execute("DELETE FROM threads WHERE thread_id = ?1", [thread_id])
                .map_err(|e| e.to_string())?;
            Ok(removed > 0)
        })
    }

    /// 新线程 id：`th_m_` 前缀（R8 服务端 history 路由 + 移动端溯源）。
    pub(crate) fn new_thread_id() -> String {
        format!("th_m_{}", uuid::Uuid::now_v7().simple())
    }

    pub(crate) fn new_message_id() -> String {
        format!("msg_{}", uuid::Uuid::now_v7().simple())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub thread_id: String,
    pub title: String,
    pub agent_id: String,
    pub updated_at: i64,
    pub message_count: usize,
    pub starred: bool,
    pub archived: bool,
}

/// ── 云同步桥（A2 API 消费方）───────────────────────────────

/// pass 前快照注入：dirty thread → `V2LocalThread`。
pub fn local_thread_snapshot(
    agent: &Arc<AgentState>,
    sync: &Arc<flowix_sync::SyncManager>,
) -> Result<Vec<V2LocalThread>, String> {
    let dirty = sync.v2_dirty_thread_ids().map_err(|e| e.to_string())?;
    let mut threads = Vec::with_capacity(dirty.len());
    for thread_id in dirty {
        match agent.thread_payload(&thread_id)? {
            Some(payload) => threads.push(V2LocalThread {
                id: thread_id,
                payload,
            }),
            // 本地已删：删除对账（delete_thread 命令）已在 dirty 队列，无需快照
            None => continue,
        }
    }
    Ok(threads)
}

/// apply：远端线程幂等并集落库。返回是否有变化（供事件聚合）。
pub fn apply_remote_thread(
    agent: &Arc<AgentState>,
    thread_id: &str,
    deleted: bool,
    content: Option<&[u8]>,
) -> Result<bool, String> {
    if deleted {
        return agent.delete_thread(thread_id);
    }
    let Some(bytes) = content else {
        return Ok(false);
    };
    let incoming: V2ThreadPayload = serde_json::from_slice(bytes)
        .map_err(|e| format!("AGENT_REMOTE_PAYLOAD_INVALID: {e}"))?;
    let merged = agent.merge_thread_payload(&incoming)?;
    let changed = agent
        .thread_payload(thread_id)?
        .is_none_or(|current| {
            serde_json::to_string(&current).ok() != serde_json::to_string(&merged).ok()
        });
    agent.save_thread_payload(&merged)?;
    Ok(changed)
}

/// 本地回合落库后的入队 + 防抖同步（fingerprint 幂等）。
pub(crate) fn enqueue_thread_change_and_sync(
    app: &tauri::AppHandle,
    agent: &Arc<AgentState>,
    sync: &Arc<flowix_sync::SyncManager>,
    payload: &V2ThreadPayload,
) {
    let Ok(json) = serde_json::to_vec(payload) else {
        return;
    };
    let fingerprint = flowix_sync::v2_content_hash(&json);
    if let Err(error) = sync.record_v2_thread_change(
        &payload.thread_id,
        flowix_sync::LocalChangeKind::Put,
        &fingerprint,
    ) {
        eprintln!("[agent] record thread change failed: {error}");
        return;
    }
    let _ = agent; // 保留参数形状与桌面桥对齐
    crate::sync::spawn_debounced_sync(app.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_normalization() {
        // 常见笔误：缺斜杠、无 scheme、尾斜杠、空白
        assert_eq!(normalize_api_base("http:api.deepseek.com"), "https://api.deepseek.com");
        assert_eq!(normalize_api_base("https:api.x.com"), "https://api.x.com");
        assert_eq!(normalize_api_base("api.deepseek.com/"), "https://api.deepseek.com");
        assert_eq!(normalize_api_base(" 8.160.162.251:8787 "), "https://8.160.162.251:8787");
        // http 明确保留、已规范不变
        assert_eq!(normalize_api_base("http://192.168.1.10:8787"), "http://192.168.1.10:8787");
        assert_eq!(normalize_api_base("https://api.deepseek.com"), "https://api.deepseek.com");
        assert_eq!(normalize_api_base(""), "");
    }

    fn temp_agent_state(tag: &str) -> (AgentState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("flowix-agent-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (AgentState::new(&dir).unwrap(), dir)
    }

    fn payload_with_id(thread_id: &str) -> V2ThreadPayload {
        V2ThreadPayload {
            version: 1,
            thread_id: thread_id.to_string(),
            agent_id: "mobile".into(),
            title: "t".into(),
            created_at: 1,
            messages: Vec::new(),
            starred: false,
            archived: false,
        }
    }

    /// 桌面同步卡存桌面本地 id（无 th_ 前缀）；本地库为云端 id（th_ 前缀）。
    /// 归一查询应命中 th_ 版本并返回归一 id；已带前缀的 id 不做二次重试。
    #[test]
    fn thread_payload_normalized_resolves_desktop_local_id() {
        let (agent, dir) = temp_agent_state("normalize");
        agent
            .save_thread_payload(&payload_with_id("th_mobile-local-abc"))
            .unwrap();

        let hit = agent
            .thread_payload_normalized("mobile-local-abc")
            .unwrap()
            .expect("unprefixed desktop id resolves");
        assert_eq!(hit.0, "th_mobile-local-abc");

        let direct = agent
            .thread_payload_normalized("th_mobile-local-abc")
            .unwrap()
            .expect("prefixed id resolves directly");
        assert_eq!(direct.0, "th_mobile-local-abc");

        assert!(agent.thread_payload_normalized("th_missing").unwrap().is_none());
        assert!(agent.thread_payload_normalized("missing").unwrap().is_none());

        std::fs::remove_dir_all(dir).ok();
    }
}
