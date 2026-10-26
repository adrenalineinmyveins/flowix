use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const PROTOCOL_EPOCH: i64 = 2;

pub fn v2_content_hash(bytes: &[u8]) -> String {
    base64_url_no_pad(&Sha256::digest(bytes))
}

/// Cloud association derived from a notebook-relative path, independent of
/// the rebuildable memo index ID and any Markdown frontmatter.
pub fn v2_path_note_id(notebook_id: &str, relative_path: &str) -> String {
    v2_content_hash(format!("{notebook_id}\0{relative_path}").as_bytes())
}

/// Decide whether a local file must be preserved instead of applying a pulled
/// Cloud head. A persisted dirty generation is authoritative. Comparing the
/// current bytes with the last acknowledged server hash also closes the short
/// watcher window between an atomic file save and dirty-queue persistence.
pub fn v2_local_content_diverged(
    local_hash: Option<&str>,
    acknowledged_hash: Option<&str>,
    has_pending_change: bool,
) -> bool {
    has_pending_change
        || matches!(
            (local_hash, acknowledged_hash),
            (Some(local), Some(acknowledged)) if local != acknowledged
        )
        || matches!((local_hash, acknowledged_hash), (Some(_), None))
}

pub fn new_v2_operation_id() -> String {
    format!("op_{}", uuid::Uuid::now_v7())
}

pub fn v2_notebook_metadata_hash(name: &str, icon: Option<&str>, sort_order: i64) -> String {
    let canonical = serde_json::to_vec(&(name, icon, sort_order))
        .expect("notebook metadata serialization is infallible");
    v2_content_hash(&canonical)
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(a >> 2) as usize] as char);
        output.push(ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(c & 0x3f) as usize] as char);
        }
    }
    output
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2CloudAccount {
    pub user: crate::models::CloudUser,
    pub protocol_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2SyncedNotebook {
    pub notebook_id: String,
    pub enabled: bool,
    pub bootstrap_required: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2NoteState {
    pub note_id: String,
    pub notebook_id: String,
    pub revision: String,
    pub content_hash: Option<String>,
    pub filename: String,
    pub deleted: bool,
    pub last_seq: i64,
    pub attachments: Vec<V2Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2NotebookState {
    pub notebook_id: String,
    pub revision: String,
    pub metadata_hash: String,
    pub deleted: bool,
    pub last_seq: i64,
}

#[derive(Debug, Clone)]
pub struct V2LocalNotebook {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub sort_order: i64,
}

#[derive(Debug, Clone)]
pub struct V2LocalNote {
    pub id: String,
    pub notebook_id: String,
    pub filename: String,
    pub content: Vec<u8>,
    pub attachments: Vec<V2LocalAttachment>,
}

// ── thread 实体（方案 A：整 Thread 单 JSON blob）──────────────

/// thread blob 内容（版本化 JSON）。桌面 thread.db 与移动端精简存储
/// 的投影；工具块为只读快照，跨端不可重放执行。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct V2ThreadPayload {
    pub version: i64,
    pub thread_id: String,
    pub agent_id: String,
    pub title: String,
    pub created_at: i64,
    #[serde(default)]
    pub messages: Vec<V2ThreadMessage>,
    /// 用户标记（P1b 会话管理）。serde default 向后兼容：旧端反序列化
    /// 忽略、重写时丢弃——因此合并规则取 local，本端标记不被远端旧版本覆盖。
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct V2MessageAttachment {
    /// 文件名（跨端按同名文件回落，与笔记图片附件同语义）
    pub file_name: String,
    /// 本设备绝对路径（attachment_save 返回；他端不可用时走回落）
    pub storage_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct V2ThreadMessage {
    /// 全局唯一（双端 uuid v7 生成），消息级合并的恒等键。
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<String>,
    #[serde(default)]
    pub tool_data: Option<String>,
    pub created_at: i64,
    /// 溯源（"desktop:pi" / "mobile" 等），纯展示用途。
    #[serde(default)]
    pub origin: Option<String>,
    /// 图片附件（P2a）：content 仍为文字；空时序列化省略，
    /// 保证旧消息字节稳定（apply 幂等/云端 hash 不漂移）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<V2MessageAttachment>,
}

/// 消息级合并：双方消息按 id 求并集（同 id 保留 local——同源生成概率≈0），
/// `(createdAt, id)` 稳定排序（跨端无全局序，弃用桌面 sequence）；
/// title 取最后消息更新的一方；thread 元数据（agentId/createdAt）以 local 为准。
/// 纯函数，供引擎 409 重推与桥接层 upsert 共用。
pub fn v2_merge_thread_payloads(
    local: &V2ThreadPayload,
    remote: &V2ThreadPayload,
) -> Result<V2ThreadPayload, String> {
    if local.thread_id != remote.thread_id {
        return Err(format!(
            "cannot merge threads with different ids: {} vs {}",
            local.thread_id, remote.thread_id
        ));
    }
    let mut merged: std::collections::BTreeMap<&str, &V2ThreadMessage> = BTreeMap::new();
    let mut local_last = 0_i64;
    for message in &local.messages {
        local_last = local_last.max(message.created_at);
        merged.insert(message.id.as_str(), message);
    }
    let mut remote_last = 0_i64;
    for message in &remote.messages {
        remote_last = remote_last.max(message.created_at);
        merged.entry(message.id.as_str()).or_insert(message);
    }
    let mut messages: Vec<V2ThreadMessage> = merged
        .into_values()
        .map(|message| (*message).clone())
        .collect();
    messages.sort_by(|left, right| (left.created_at, &left.id).cmp(&(right.created_at, &right.id)));
    // title 冲突取"内容更新"的一方：以双方最后一条消息的时间为判据，
    // 相等（含双方都无消息）时保留 local。
    let title = if remote_last > local_last {
        remote.title.clone()
    } else {
        local.title.clone()
    };
    Ok(V2ThreadPayload {
        version: local.version.max(remote.version),
        thread_id: local.thread_id.clone(),
        agent_id: local.agent_id.clone(),
        title,
        created_at: local.created_at,
        messages,
        // 用户标记取 local（见结构体注释：旧端重写会丢字段，local 优先保本端视图）
        starred: local.starred,
        archived: local.archived,
    })
}

/// 引擎 freeze 输入：本地线程快照（payload 序列化后为 blob 内容）。
#[derive(Debug, Clone)]
pub struct V2LocalThread {
    pub id: String,
    pub payload: V2ThreadPayload,
}

/// thread 云端基线（照 V2NoteState，不含 notebook 归属）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2ThreadState {
    pub thread_id: String,
    pub revision: String,
    pub content_hash: Option<String>,
    pub title: String,
    pub deleted: bool,
    pub last_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct V2Attachment {
    pub filename: String,
    pub content_hash: String,
    pub size_bytes: i64,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct V2LocalAttachment {
    pub metadata: V2Attachment,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct V2RemoteAttachment {
    pub metadata: V2Attachment,
    pub content: Vec<u8>,
}

/// Collects files referenced by legacy asset URLs, relative Markdown links,
/// and Obsidian wiki embeds.
/// `attachments/` is notebook-scoped storage, while cloud manifests must be
/// document-scoped to avoid unrelated files causing sync churn.
pub fn collect_v2_attachments(
    directory: &Path,
    markdown: &[u8],
) -> Result<Vec<V2LocalAttachment>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let directory = match std::fs::canonicalize(directory) {
        Ok(directory) => directory,
        // The attachments directory may be removed between `exists` and
        // canonicalization. Treat that the same as an empty directory.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut attachments = Vec::new();
    for path in referenced_attachment_paths(&directory, markdown) {
        let filename = path
            .strip_prefix(&directory)
            .ok()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .replace('\\', "/");
        if filename.is_empty() || filename.contains(['/', '\\']) {
            continue;
        }
        let content = match std::fs::read(&path) {
            Ok(content) => content,
            // An attachment can disappear after the markdown scan. It should
            // not make the entire note/account sync fail; the next sync can
            // upload it again if it is recreated locally.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        attachments.push(V2LocalAttachment {
            metadata: V2Attachment {
                mime_type: attachment_mime_type(&filename).to_string(),
                filename,
                content_hash: v2_content_hash(&content),
                size_bytes: i64::try_from(content.len())
                    .map_err(|_| "attachment exceeds i64".to_string())?,
            },
            content,
        });
    }
    attachments.sort_by(|left, right| left.metadata.filename.cmp(&right.metadata.filename));
    Ok(attachments)
}

pub fn referenced_attachment_paths(directory: &Path, markdown: &[u8]) -> BTreeSet<PathBuf> {
    const PREFIXES: [&str; 3] = [
        "asset://localhost/",
        "http://asset.localhost/",
        "https://asset.localhost/",
    ];
    let source = String::from_utf8_lossy(markdown);
    let mut paths = BTreeSet::new();
    for prefix in PREFIXES {
        let mut remaining = source.as_ref();
        while let Some(index) = remaining.find(prefix) {
            let encoded = &remaining[index + prefix.len()..];
            let end = encoded
                .find(|value: char| {
                    value.is_whitespace() || matches!(value, ')' | '"' | '\'' | '<' | '>')
                })
                .unwrap_or(encoded.len());
            if let Some(decoded) = percent_decode(&encoded[..end]) {
                if let Ok(path) = std::fs::canonicalize(PathBuf::from(decoded)) {
                    if path.starts_with(directory) && path.is_file() {
                        paths.insert(path);
                    }
                }
            }
            remaining = &encoded[end..];
            if remaining.is_empty() {
                break;
            }
        }
    }
    let notebook_root = directory.parent().unwrap_or(directory);
    let mut candidates = Vec::new();
    let mut rest = source.as_ref();
    while let Some(start) = rest.find("![[") {
        let tail = &rest[start + 3..];
        if let Some(end) = tail.find("]]") {
            candidates.push(
                tail[..end]
                    .split('|')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            );
            rest = &tail[end + 2..];
        } else {
            break;
        }
    }
    for token in source.split_whitespace() {
        let token = token.trim_matches(|c: char| matches!(c, '(' | ')' | '"' | '\'' | '<' | '>'));
        if token.starts_with("./attachments/") || token.starts_with("attachments/") {
            candidates.push(token.trim_start_matches("./").to_string());
        }
    }
    for candidate in candidates {
        if let Ok(path) = std::fs::canonicalize(notebook_root.join(candidate)) {
            if path.starts_with(directory) && path.is_file() {
                paths.insert(path);
            }
        }
    }
    paths
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            decoded.push(
                (hex_value(*bytes.get(index + 1)?)? << 4) | hex_value(*bytes.get(index + 2)?)?,
            );
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn attachment_mime_type(filename: &str) -> &'static str {
    match Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, Clone)]
pub enum V2RemoteApply {
    Notebook {
        notebook_id: String,
        name: Option<String>,
        icon: Option<String>,
        sort_order: Option<i64>,
        revision: String,
        sync_seq: i64,
        deleted: bool,
    },
    Note {
        note_id: String,
        notebook_id: String,
        filename: String,
        content_hash: Option<String>,
        content: Option<Vec<u8>>,
        revision: String,
        sync_seq: i64,
        deleted: bool,
        attachments: Vec<V2RemoteAttachment>,
    },
    /// 远端（或 409 合并产物）线程到达本端。content 为 payload JSON 字节，
    /// 由桌面/移动桥 upsert 到本地线程存储（消息按 id 幂等）。
    Thread {
        thread_id: String,
        agent_id: String,
        title: String,
        content_hash: Option<String>,
        content: Option<Vec<u8>>,
        revision: String,
        sync_seq: i64,
        deleted: bool,
    },
}

#[derive(Debug, Clone, Default)]
pub struct V2AccountSyncReport {
    pub(crate) auth_generation: Option<u64>,
    pub started_at: i64,
    pub cursor: i64,
    pub head_cursor: i64,
    pub uploaded: usize,
    pub deleted: usize,
    pub remote: Vec<V2RemoteApply>,
    pub bootstrapped_notebooks: Vec<String>,
    pub bootstrap_next_page_token: Option<String>,
    /// 本轮 bootstrap 已含全部 threads（A2）；commit 时清除
    /// threads_bootstrap_required 标志。apply 失败则下轮重新 bootstrap。
    pub threads_bootstrapped: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum V2EntityType {
    Notebook,
    Note,
    /// AI 对话线程（整线程消息 JSON 为一个内容寻址 blob）。
    /// A1 起服务端支持；客户端引擎接入在 A2。
    Thread,
}

impl V2EntityType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Notebook => "notebook",
            Self::Note => "note",
            Self::Thread => "thread",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "notebook" => Some(Self::Notebook),
            "note" => Some(Self::Note),
            "thread" => Some(Self::Thread),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum V2OperationKind {
    Put,
    Delete,
}

impl V2OperationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "put" => Some(Self::Put),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2DirtyEntity {
    pub entity_type: V2EntityType,
    pub entity_id: String,
    pub notebook_id: Option<String>,
    pub generation: i64,
    pub operation_kind: V2OperationKind,
    pub fingerprint: String,
    pub detected_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2InflightOperation {
    pub operation_id: String,
    pub entity_type: V2EntityType,
    pub entity_id: String,
    pub generation: i64,
    pub operation_kind: V2OperationKind,
    pub base_revision: Option<String>,
    pub payload_json: String,
    pub attempts: i64,
    pub next_retry_at: i64,
}

#[derive(Debug, Clone)]
pub struct V2FreezeOperation<'a> {
    pub operation_id: &'a str,
    pub entity_type: V2EntityType,
    pub entity_id: &'a str,
    pub generation: i64,
    pub operation_kind: V2OperationKind,
    pub base_revision: Option<&'a str>,
    pub payload_json: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2SyncStatus {
    pub protocol_epoch: i64,
    pub cursor: i64,
    pub head_cursor: i64,
    pub has_changes: bool,
    pub server_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2Change {
    pub sync_seq: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub notebook_id: Option<String>,
    pub revision: String,
    pub kind: String,
    pub created_at: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<i64>,
    pub deleted: bool,
    #[serde(default)]
    pub attachments: Vec<V2Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2ChangesPage {
    pub protocol_epoch: i64,
    pub cursor: i64,
    pub head_cursor: i64,
    pub has_more: bool,
    pub changes: Vec<V2Change>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Bootstrap {
    pub protocol_epoch: i64,
    pub cursor: i64,
    pub server_time: i64,
    pub notebooks: Vec<V2BootstrapNotebook>,
    pub notes: Vec<V2BootstrapNote>,
    /// AI 对话线程（A1 服务端起输出；`#[serde(default)]` 保证新客户端
    /// 可读旧服务端、旧客户端反序列化自然忽略）。
    #[serde(default)]
    pub threads: Vec<V2BootstrapThread>,
    pub usage: V2Usage,
    #[serde(default)]
    pub next_page_token: Option<String>,
}

/// bootstrap 输出的线程快照行（消息体按 content_hash 走 blob 下载）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BootstrapThread {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub revision: String,
    pub content_hash: Option<String>,
    pub size_bytes: i64,
    pub deleted: bool,
    pub sync_seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2History {
    pub note_id: String,
    pub notebook_id: String,
    pub current_revision: String,
    pub revisions: Vec<V2HistoryRevision>,
    #[serde(default)]
    pub next_before_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2HistoryRevision {
    pub revision: String,
    pub deleted: bool,
    pub relative_path: Option<String>,
    pub content_hash: Option<String>,
    pub size_bytes: i64,
    pub sync_seq: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BootstrapNotebook {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub sort_order: i64,
    pub revision: String,
    pub deleted: bool,
    pub sync_seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BootstrapNote {
    pub id: String,
    pub notebook_id: String,
    pub filename: String,
    #[serde(default)]
    pub moved_from_note_id: Option<String>,
    pub revision: String,
    pub content_hash: Option<String>,
    pub size_bytes: i64,
    pub deleted: bool,
    pub sync_seq: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub attachments: Vec<V2Attachment>,
}

#[derive(Debug, Clone)]
pub struct V2ConflictMaterial {
    pub notebook_id: String,
    pub note_id: String,
    pub operation_id: String,
    pub remote: V2BootstrapNote,
    pub relocated: Option<V2BootstrapNote>,
    pub base_content: Option<Vec<u8>>,
    pub remote_content: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2PendingMove {
    pub operation_id: String,
    pub notebook_id: String,
    pub from_note_id: String,
    pub from_path: String,
    pub to_note_id: String,
    pub to_path: String,
    pub base_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Usage {
    pub used_bytes: i64,
    pub quota_bytes: i64,
    pub entitlement_expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BlobReservation {
    pub reservation_id: String,
    pub content_hash: String,
    pub size_bytes: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BlobReservationEnvelope {
    pub data: V2BlobReservation,
    pub upload: V2BlobUpload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BlobUpload {
    pub method: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub completion_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BlobDownloadEnvelope {
    pub data: V2BlobDownload,
    pub download: V2BlobDownloadCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BlobDownload {
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2BlobDownloadCapability {
    pub method: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum V2PushOperation {
    #[serde(rename = "notebook.put")]
    NotebookPut {
        operation_id: String,
        base_revision: Option<String>,
        notebook: V2NotebookPut,
    },
    #[serde(rename = "notebook.delete")]
    NotebookDelete {
        operation_id: String,
        base_revision: Option<String>,
        notebook_id: String,
        base_tree_seq: i64,
    },
    #[serde(rename = "note.put")]
    NotePut {
        operation_id: String,
        base_revision: Option<String>,
        note: V2NotePut,
    },
    #[serde(rename = "note.delete")]
    NoteDelete {
        operation_id: String,
        base_revision: Option<String>,
        note_id: String,
    },
    #[serde(rename = "note.move")]
    NoteMove {
        operation_id: String,
        base_revision: Option<String>,
        notebook_id: String,
        from_note_id: String,
        from_path: String,
        to_note_id: String,
        to_path: String,
    },
    #[serde(rename = "thread.put")]
    ThreadPut {
        operation_id: String,
        base_revision: Option<String>,
        thread: V2ThreadPut,
    },
    #[serde(rename = "thread.delete")]
    ThreadDelete {
        operation_id: String,
        base_revision: Option<String>,
        thread_id: String,
    },
}

/// thread.put 的线程元数据（消息体走内容寻址 blob，与 note 同款）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2ThreadPut {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub content_hash: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2NotebookPut {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2NotePut {
    pub id: String,
    pub notebook_id: String,
    pub filename: String,
    pub file_kind: String,
    pub content_hash: String,
    pub size_bytes: i64,
    #[serde(default)]
    pub attachments: Vec<V2Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2OperationResult {
    pub operation_id: String,
    pub ok: bool,
    pub status: u16,
    #[serde(default)]
    pub data: Option<V2OperationData>,
    #[serde(default)]
    pub error: Option<V2OperationError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2OperationError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2OperationData {
    pub entity_type: String,
    pub entity_id: String,
    pub revision: String,
    pub sync_seq: i64,
    pub deleted: bool,
    pub resolution: String,
    pub base_revision: Option<String>,
    pub replaced_revision: Option<String>,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub source_revision: Option<String>,
    #[serde(default)]
    pub source_sync_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V2PushResult {
    pub protocol_epoch: i64,
    pub results: Vec<V2OperationResult>,
    pub head_cursor: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        collect_v2_attachments, new_v2_operation_id, v2_content_hash,
        v2_local_content_diverged, v2_merge_thread_payloads, v2_path_note_id, V2MessageAttachment,
        V2NotePut, V2PushOperation, V2ThreadMessage, V2ThreadPayload,
    };

    fn thread_message(id: &str, role: &str, created_at: i64) -> V2ThreadMessage {
        V2ThreadMessage {
            id: id.into(),
            role: role.into(),
            content: format!("content-{id}"),
            reasoning: None,
            tool_name: None,
            tool_input: None,
            tool_data: None,
            created_at,
            origin: None,
            attachments: Vec::new(),
        }
    }

    fn thread_payload(id: &str, title: &str, messages: Vec<V2ThreadMessage>) -> V2ThreadPayload {
        V2ThreadPayload {
            version: 1,
            thread_id: id.into(),
            agent_id: "builtin.chat".into(),
            title: title.into(),
            created_at: 1,
            messages,
            starred: false,
            archived: false,
        }
    }

    #[test]
    fn thread_merge_takes_message_id_union_sorted_by_created_at() {
        // 双端交错并发：A 持 msg_1/msg_2，B 持 msg_1（同源）/msg_3（并发新增）
        let local = thread_payload(
            "th_x",
            "本地标题",
            vec![
                thread_message("msg_1", "user", 100),
                thread_message("msg_2", "assistant", 200),
            ],
        );
        let remote = thread_payload(
            "th_x",
            "远端标题",
            vec![
                thread_message("msg_1", "user", 100),
                thread_message("msg_3", "assistant", 150),
            ],
        );
        let merged = v2_merge_thread_payloads(&local, &remote).unwrap();
        let ids: Vec<&str> = merged.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["msg_1", "msg_3", "msg_2"], "并集 + (createdAt,id) 序");
        // remote 最后消息（150）早于 local（200）→ title 取 local
        assert_eq!(merged.title, "本地标题");
        assert_eq!(merged.agent_id, "builtin.chat");
    }

    #[test]
    fn thread_merge_prefers_remote_title_when_remote_is_newer() {
        let local = thread_payload(
            "th_x",
            "旧标题",
            vec![thread_message("msg_1", "user", 100)],
        );
        let remote = thread_payload(
            "th_x",
            "新标题",
            vec![
                thread_message("msg_1", "user", 100),
                thread_message("msg_9", "assistant", 900),
            ],
        );
        let merged = v2_merge_thread_payloads(&local, &remote).unwrap();
        assert_eq!(merged.title, "新标题");
        assert_eq!(merged.messages.len(), 2);
    }

    #[test]
    fn thread_merge_keeps_local_starred_archived_flags() {
        // P1b：用户标记取 local——旧端重写会丢字段，local 优先保本端视图
        let local = {
            let mut payload = thread_payload("th_x", "本地", vec![thread_message("msg_1", "user", 100)]);
            payload.starred = true;
            payload
        };
        // 远端（旧版本端重写）无标记
        let remote = thread_payload(
            "th_x",
            "远端标题",
            vec![
                thread_message("msg_1", "user", 100),
                thread_message("msg_9", "assistant", 900),
            ],
        );
        let merged = v2_merge_thread_payloads(&local, &remote).unwrap();
        assert!(merged.starred, "本端收藏不被远端旧版本覆盖");
        assert!(!merged.archived);
    }

    #[test]
    fn thread_merge_breaks_created_at_ties_by_message_id() {
        // 同刻双发（R3）：createdAt 相等时按 id 稳定排序
        let local = thread_payload(
            "th_x",
            "l",
            vec![thread_message("msg_b", "user", 500)],
        );
        let remote = thread_payload(
            "th_x",
            "r",
            vec![thread_message("msg_a", "assistant", 500)],
        );
        let merged = v2_merge_thread_payloads(&local, &remote).unwrap();
        let ids: Vec<&str> = merged.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["msg_a", "msg_b"]);
    }

    #[test]
    fn thread_merge_rejects_cross_thread_merge() {
        let left = thread_payload("th_1", "a", Vec::new());
        let right = thread_payload("th_2", "b", Vec::new());
        assert!(v2_merge_thread_payloads(&left, &right).is_err());
    }

    #[test]
    fn thread_merge_scales_to_a_thousand_interleaved_messages() {
        let mut local_messages = Vec::new();
        let mut remote_messages = Vec::new();
        for index in 0..1000 {
            if index % 2 == 0 {
                local_messages.push(thread_message(&format!("msg_l_{index}"), "user", index));
            } else {
                remote_messages.push(thread_message(&format!("msg_r_{index}"), "user", index));
            }
        }
        let local = thread_payload("th_big", "big", local_messages);
        let remote = thread_payload("th_big", "big", remote_messages);
        let merged = v2_merge_thread_payloads(&local, &remote).unwrap();
        assert_eq!(merged.messages.len(), 1000, "交错千条零丢失");
        assert!(merged.messages.windows(2).all(|pair| {
            (pair[0].created_at, &pair[0].id) <= (pair[1].created_at, &pair[1].id)
        }));
    }

    #[test]
    fn thread_payload_roundtrips_camel_case_json() {
        let payload = thread_payload(
            "th_wire",
            "Wire",
            vec![V2ThreadMessage {
                id: "msg_w".into(),
                role: "tool".into(),
                content: "done".into(),
                reasoning: Some("thinking".into()),
                tool_name: Some("search".into()),
                tool_input: Some("{}".into()),
                tool_data: Some("<result>".into()),
                created_at: 42,
                origin: Some("desktop:pi".into()),
                attachments: vec![V2MessageAttachment {
                    file_name: "shot.png".into(),
                    storage_key: "/data/attachments/shot.png".into(),
                }],
            }],
        );
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["threadId"], "th_wire");
        assert_eq!(json["messages"][0]["toolName"], "search");
        assert_eq!(json["messages"][0]["createdAt"], 42);
        assert_eq!(json["messages"][0]["attachments"][0]["fileName"], "shot.png");
        let parsed: V2ThreadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, payload);
    }

    #[test]
    fn thread_message_without_attachments_omits_field() {
        // P2a：无附件消息序列化不含 attachments 键（旧消息字节稳定）
        let message = thread_message("msg_n", "user", 1);
        let json = serde_json::to_value(&message).unwrap();
        assert!(json.get("attachments").is_none());
    }

    #[test]
    fn thread_payload_tolerates_missing_optional_fields() {
        // 旧端/未来端消息缺 optional 字段（serde default）不炸反序列化
        let json = serde_json::json!({
            "version": 1,
            "threadId": "th_min",
            "agentId": "agent_loop",
            "title": "t",
            "createdAt": 1,
            "messages": [{ "id": "m1", "role": "user", "content": "hi", "createdAt": 2 }]
        });
        let parsed: V2ThreadPayload = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.messages.len(), 1);
        assert!(parsed.messages[0].origin.is_none());
        assert!(parsed.messages[0].tool_name.is_none());
    }

    #[test]
    fn cloud_note_id_tracks_notebook_relative_path_without_memo_id() {
        assert_eq!(
            v2_path_note_id("nb_test", "Projects/Plan.md"),
            v2_content_hash(b"nb_test\0Projects/Plan.md"),
        );
        assert_ne!(
            v2_path_note_id("nb_test", "Projects/Plan.md"),
            v2_path_note_id("nb_test", "Archive/Plan.md"),
        );
    }

    #[test]
    fn content_hash_matches_web_crypto_base64_url_without_padding() {
        assert_eq!(
            v2_content_hash(b""),
            "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"
        );
        assert_eq!(v2_content_hash(b"Flowix").len(), 43);
    }

    #[test]
    fn operation_ids_are_unique_and_protocol_safe() {
        let first = new_v2_operation_id();
        let second = new_v2_operation_id();
        assert_ne!(first, second);
        assert!(first.starts_with("op_"));
        assert!(first
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '-'));
    }

    #[test]
    fn local_divergence_covers_dirty_and_watcher_settle_windows() {
        assert!(!v2_local_content_diverged(
            Some("same"),
            Some("same"),
            false
        ));
        assert!(v2_local_content_diverged(
            Some("local"),
            Some("cloud"),
            false
        ));
        assert!(v2_local_content_diverged(Some("new"), None, false));
        assert!(v2_local_content_diverged(None, Some("cloud"), true));
        assert!(!v2_local_content_diverged(None, Some("cloud"), false));
    }

    #[test]
    fn push_operation_matches_the_cloud_camel_case_contract() {
        let value = serde_json::to_value(V2PushOperation::NotePut {
            operation_id: "op_test_123".into(),
            base_revision: Some("rev_1".into()),
            note: V2NotePut {
                id: "abc12345".into(),
                notebook_id: "nb_0198f1aa-7b22-7def-8123-0123456789ab".into(),
                filename: "abc12345.md".into(),
                file_kind: "markdown".into(),
                content_hash: "A".repeat(43),
                size_bytes: 12,
                attachments: Vec::new(),
            },
        })
        .unwrap();
        assert_eq!(value["type"], "note.put");
        assert_eq!(value["operationId"], "op_test_123");
        assert_eq!(value["baseRevision"], "rev_1");
        assert_eq!(
            value["note"]["notebookId"],
            "nb_0198f1aa-7b22-7def-8123-0123456789ab"
        );
        assert_eq!(value["note"]["sizeBytes"], 12);
        assert!(value.get("operation_id").is_none());
        let deletion = serde_json::to_value(V2PushOperation::NotebookDelete {
            operation_id: "op_delete_123".into(),
            base_revision: Some("rev_1".into()),
            notebook_id: "nb_1".into(),
            base_tree_seq: 42,
        })
        .unwrap();
        assert_eq!(deletion["baseTreeSeq"], 42);
    }

    #[test]
    fn attachment_manifest_contains_only_document_asset_links() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("attachments");
        std::fs::create_dir(&directory).unwrap();
        let used = directory.join("used image.png");
        std::fs::write(&used, b"used").unwrap();
        std::fs::write(directory.join("unrelated.mp4"), b"unrelated").unwrap();
        let encoded = used
            .to_string_lossy()
            .replace('/', "%2F")
            .replace(' ', "%20");
        let markdown = format!("![image](asset://localhost/{encoded})");

        let attachments = collect_v2_attachments(&directory, markdown.as_bytes()).unwrap();

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].metadata.filename, "used image.png");
        assert_eq!(attachments[0].metadata.mime_type, "image/png");
    }

    #[test]
    fn attachment_manifest_rejects_paths_outside_the_notebook_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("attachments");
        std::fs::create_dir(&directory).unwrap();
        let outside = temp.path().join("private.pdf");
        std::fs::write(&outside, b"private").unwrap();
        let encoded = outside.to_string_lossy().replace('/', "%2F");
        let markdown = format!("[file](asset://localhost/{encoded})");

        assert!(collect_v2_attachments(&directory, markdown.as_bytes())
            .unwrap()
            .is_empty());
    }
}
