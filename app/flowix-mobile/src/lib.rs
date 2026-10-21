//! Flowix 安卓壳。
//!
//! M0.3：Tauri 2 mobile 壳 + WebView + flowix-core 验证。
//! M1：flowix-sync 接入（认证/缓存适配/同步回路）。
//! A4：AI 对话（OpenAI 兼容 SSE 引擎 + 会话存储 + 线程云同步）。

mod agent;
mod cache;
mod cloud;
mod commands;
mod sync;

use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let cache_root = dir.join("notebooks");
            std::fs::create_dir_all(&cache_root)?;
            app.manage(cache::MobileState {
                memo_file: flowix_core::memo_file::MemoFile::new(dir.clone()),
                cache_root,
            });
            let sync = cloud::setup(&app.handle(), dir.clone())?;
            sync::spawn_auto_sync(app.handle().clone(), sync.clone());
            app.manage(sync);
            let agent = Arc::new(agent::AgentState::new(&dir)?);
            app.manage(agent);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::init_cache,
            commands::cloud_login,
            commands::cloud_register,
            commands::cloud_logout,
            commands::cloud_get_state,
            commands::cloud_refresh_notebooks,
            commands::cloud_sync_now,
            commands::notebook_list,
            commands::note_list,
            commands::note_read,
            commands::note_save,
            commands::note_create,
            commands::note_delete,
            commands::attachment_save,
            commands::attachment_read,
            commands::note_history,
            commands::note_history_read,
            commands::app_check_update,
            commands::app_open_url,
            commands::search,
            agent::commands::agent_provider_get,
            agent::commands::agent_provider_set,
            agent::commands::agent_thread_list,
            agent::commands::agent_thread_messages,
            agent::commands::agent_thread_delete,
            agent::commands::agent_chat_send,
            agent::commands::agent_chat_stop
        ])
        .run(tauri::generate_context!())
        .expect("error while running flowix mobile");
}
