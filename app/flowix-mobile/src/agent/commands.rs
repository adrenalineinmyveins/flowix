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
    Ok(agent
        .thread_payload(&thread_id)?
        .map(|payload| payload.messages)
        .unwrap_or_default())
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

/// 发送消息并流式接收。立即返回 thread_id；进度走 `agent-chunk` 事件。
#[tauri::command]
pub async fn agent_chat_send(
    app: AppHandle,
    agent: State<'_, Arc<AgentState>>,
    sync: State<'_, Arc<flowix_sync::SyncManager>>,
    thread_id: Option<String>,
    content: String,
) -> Result<String, String> {
    super::chat::chat_send(app, agent.inner().clone(), sync.inner().clone(), thread_id, content)
        .await
}

#[tauri::command]
pub fn agent_chat_stop(thread_id: String) {
    super::chat::request_stop(&thread_id);
}
