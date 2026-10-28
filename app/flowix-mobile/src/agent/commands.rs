//! agent_* IPC 命令（方案 A4）。

use super::{AgentProviderConfig, AgentState, ThreadSummary, THREADS_CHANGED_EVENT};
use flowix_sync::V2ThreadMessage;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

fn emit_threads_changed(app: &AppHandle) {
    let _ = app.emit(THREADS_CHANGED_EVENT, ());
}

#[tauri::command]
pub fn agent_provider_get(agent: State<'_, Arc<AgentState>>) -> Result<AgentProviderConfig, String> {
    agent.provider_config()
}

#[tauri::command]
pub fn agent_provider_set(
    app: AppHandle,
    agent: State<'_, Arc<AgentState>>,
    config: AgentProviderConfig,
) -> Result<(), String> {
    agent.save_provider_config(&config)
}

#[tauri::command]
pub fn agent_thread_list(agent: State<'_, Arc<AgentState>>) -> Result<Vec<ThreadSummary>, String> {
    agent.thread_summaries()
}

#[tauri::command]
pub fn agent_thread_messages(
    agent: State<'_, Arc<AgentState>>,
    thread_id: String,
) -> Result<Vec<V2ThreadMessage>, String> {
    // 桌面同步卡携带无 th_ 前缀的桌面本地 id：归一查询（thread_payload_normalized）
    Ok(agent
        .thread_payload_normalized(&thread_id)?
        .map(|(_, payload)| payload.messages)
        .unwrap_or_default())
}

/// D3 历史分页：按 (createdAt, id) 双键游标取最新 limit 条（升序返回）。
/// 双键避免同毫秒消息被分页边界丢掉。缺省/旧调用走 `agent_thread_messages` 全量。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMessagesPage {
    pub messages: Vec<V2ThreadMessage>,
    pub has_more: bool,
    pub oldest_created_at: Option<i64>,
    pub oldest_id: Option<String>,
}

#[tauri::command]
pub fn agent_thread_messages_page(
    agent: State<'_, Arc<AgentState>>,
    thread_id: String,
    limit: Option<i64>,
    before_created_at: Option<i64>,
    before_id: Option<String>,
) -> Result<ThreadMessagesPage, String> {
    let Some((_, payload)) = agent.thread_payload_normalized(&thread_id)? else {
        return Ok(ThreadMessagesPage {
            messages: Vec::new(),
            has_more: false,
            oldest_created_at: None,
            oldest_id: None,
        });
    };
    let mut messages = payload.messages;
    messages.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    let cursor = match (before_created_at, before_id) {
        (Some(created_at), Some(id)) => Some((created_at, id)),
        _ => None,
    };
    let eligible: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            cursor
                .as_ref()
                .is_none_or(|(ts, id)| (&message.created_at, &message.id) < (ts, id))
        })
        .map(|(index, _)| index)
        .collect();
    let limit = limit.unwrap_or(50).clamp(1, 200) as usize;
    let take = eligible.len().min(limit);
    let has_more = eligible.len() > take;
    let page: Vec<V2ThreadMessage> = eligible[eligible.len() - take..]
        .iter()
        .map(|&index| messages[index].clone())
        .collect();
    let oldest = page.first().map(|message| (message.created_at, message.id.clone()));
    let (oldest_created_at, oldest_id) = oldest
        .map(|(created_at, id)| (Some(created_at), Some(id)))
        .unwrap_or((None, None));
    Ok(ThreadMessagesPage {
        messages: page,
        has_more,
        oldest_created_at,
        oldest_id,
    })
}

#[tauri::command]
pub async fn agent_thread_delete(
    app: AppHandle,
    agent: State<'_, Arc<AgentState>>,
    sync: State<'_, Arc<flowix_sync::SyncManager>>,
    thread_id: String,
) -> Result<bool, String> {
    let removed = agent.delete_thread(&thread_id)?;
    if removed {
        sync.record_v2_thread_change(
            &thread_id,
            flowix_sync::LocalChangeKind::Delete,
            "deleted",
        )
        .map_err(|e| e.to_string())?;
        emit_threads_changed(&app);
        crate::sync::spawn_debounced_sync(app);
    }
    Ok(removed)
}

/// 会话元数据更新（重命名/收藏/归档，None 字段不动；P1b）。
#[tauri::command]
pub async fn agent_thread_update(
    app: AppHandle,
    agent: State<'_, Arc<AgentState>>,
    sync: State<'_, Arc<flowix_sync::SyncManager>>,
    thread_id: String,
    title: Option<String>,
    starred: Option<bool>,
    archived: Option<bool>,
) -> Result<(), String> {
    agent
        .update_thread_meta(&thread_id, title.as_deref(), starred, archived)?
        .ok_or_else(|| format!("AGENT_THREAD_NOT_FOUND: {thread_id}"))?;
    sync.record_v2_thread_change(
        &thread_id,
        flowix_sync::LocalChangeKind::Put,
        "meta-updated",
    )
    .map_err(|e| e.to_string())?;
    emit_threads_changed(&app);
    crate::sync::spawn_debounced_sync(app);
    Ok(())
}

/// 卡片所在笔记（P1a 上下文注入）。嵌套结构体字段不随命令参数做
/// camelCase 自动转换，需显式声明（前端传 notebookId/relativePath）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNoteContext {
    pub notebook_id: String,
    pub relative_path: String,
}

/// 消息图片附件（P2a）：attachment_save 的产物随消息上送。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttachmentInput {
    pub file_name: String,
    pub storage_key: String,
}

/// 发送消息并流式接收。立即返回 thread_id；进度走 `agent-chunk` 事件。
#[tauri::command]
pub async fn agent_chat_send(
    app: AppHandle,
    agent: State<'_, Arc<AgentState>>,
    sync: State<'_, Arc<flowix_sync::SyncManager>>,
    state: State<'_, crate::cache::MobileState>,
    thread_id: Option<String>,
    content: String,
    note_context: Option<AgentNoteContext>,
    attachments: Option<Vec<AgentAttachmentInput>>,
) -> Result<String, String> {
    let note_context = note_context.map(|context| super::chat::ChatNoteContext {
        notebook_id: context.notebook_id,
        relative_path: context.relative_path,
    });
    let attachments = attachments
        .unwrap_or_default()
        .into_iter()
        .map(|input| flowix_sync::V2MessageAttachment {
            file_name: input.file_name,
            storage_key: input.storage_key,
        })
        .collect();
    super::chat::chat_send(
        app,
        agent.inner().clone(),
        sync.inner().clone(),
        state.inner(),
        thread_id,
        content,
        note_context,
        attachments,
    )
    .await
}

#[tauri::command]
pub fn agent_chat_stop(thread_id: String) {
    // 与 chat_send 同口径归一（桌面同步卡存无前缀本地 id），确保命中
    // 运行中回合的停止位
    let thread_id = if thread_id.starts_with("th_") || thread_id.is_empty() {
        thread_id
    } else {
        format!("th_{thread_id}")
    };
    super::chat::request_stop(&thread_id);
}
