//! Mobile chat runtime ── 桌面侧的 OpenAI 兼容 SSE 直连引擎（方案 A 收尾）。
//!
//! 与外部 CLI runtime 的关键差异：
//! - 无子进程：LLM 走 HTTP SSE 直连（引擎移植自 flowix-mobile A4 `agent/chat.rs`）；
//! - 消息持久化走 `thread_messages`（产品线程），不是 `agent_external_events`
//!   （C7 排除面）——回合结束经 A3 桥「pass 前对账」自动上云，与移动端
//!   会话共用同一 blob，双端并集续聊。
//! - provider 配置平文件 `<config_dir>/mobile-agent-provider.json`
//!   （API key 不上云，与移动端 `agent_provider.json` 同构）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use super::runtime_registry::{ExternalCliRuntime, ExternalRuntimeKind};
use crate::agent_session::types::ChatMessage;
use crate::agent_session::ThreadManager;
use crate::agent_types::AgentId;
use crate::agent_wire::{AgentUserMessage, RunInfo};

pub const AGENT_TYPE: &str = "mobile";

const PROVIDER_FILE: &str = "mobile-agent-provider.json";
/// SSE 两个数据帧之间的最大间隔（对齐移动端 A4）。
const CHUNK_GAP_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MobileProviderConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl MobileProviderConfig {
    pub fn is_ready(&self) -> bool {
        !(self.base_url.trim().is_empty()
            || self.api_key.trim().is_empty()
            || self.model.trim().is_empty())
    }

    fn chat_completions_url(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            return base.to_string();
        }
        if base.ends_with("/v1") {
            return format!("{base}/chat/completions");
        }
        format!("{base}/v1/chat/completions")
    }
}

struct MobileRun {
    run_id: String,
    stop: Arc<AtomicBool>,
    started_at: i64,
}

pub struct MobileChatManager {
    thread_manager: Arc<ThreadManager>,
    provider_path: PathBuf,
    runs: Mutex<HashMap<String, MobileRun>>,
}

impl MobileChatManager {
    pub fn new(thread_manager: Arc<ThreadManager>, config_dir: PathBuf) -> Self {
        Self {
            thread_manager,
            provider_path: config_dir.join(PROVIDER_FILE),
            runs: Mutex::new(HashMap::new()),
        }
    }

    pub fn provider_config(&self) -> Result<MobileProviderConfig, String> {
        match std::fs::read_to_string(&self.provider_path) {
            // Windows 编辑器（记事本/PS5.1 Set-Content）常写 UTF-8 BOM，serde_json 不容忍，先剥掉。
            Ok(content) => serde_json::from_str(content.trim_start_matches('\u{feff}'))
                .map_err(|error| format!("MOBILE_PROVIDER_CONFIG_INVALID: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(MobileProviderConfig::default())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn set_provider_config(&self, config: &MobileProviderConfig) -> Result<(), String> {
        let content =
            serde_json::to_string(config).map_err(|error| error.to_string())?;
        if let Some(parent) = self.provider_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&self.provider_path, content).map_err(|error| error.to_string())
    }

    fn message(
        id: &str,
        role: &str,
        content: String,
        reasoning: Option<String>,
    ) -> ChatMessage {
        let timestamp = chrono::DateTime::from_timestamp_millis(
            chrono::Utc::now().timestamp_millis(),
        )
        .map(|value| value.to_rfc3339())
        .unwrap_or_default();
        ChatMessage {
            id: id.into(),
            role: role.into(),
            message_type: None,
            content,
            attachments: None,
            llm_content: None,
            system_reminder_directory: None,
            timestamp,
            source_sequence: None,
            is_loading: None,
            tool_call_id: None,
            tool_name: None,
            tool_data: None,
            tool_input: None,
            tool_call: None,
            tool_result: None,
            tool_calls: None,
            reasoning,
            is_completed: None,
            error_details: None,
            is_collapsed: None,
            codex_turn_id: None,
            turn_duration_ms: None,
        }
    }

    pub async fn chat_stream(
        self: &Arc<Self>,
        thread_id: &str,
        message: AgentUserMessage,
        app_handle: &tauri::AppHandle,
    ) -> Result<String, String> {
        let config = self.provider_config()?;
        if !config.is_ready() {
            return Err("MOBILE_PROVIDER_NOT_CONFIGURED".into());
        }
        let content = message.content.trim().to_string();
        if content.is_empty() {
            return Err("MOBILE_CHAT_EMPTY_MESSAGE".into());
        }
        let run_id = super::shared::resolve_run_id(thread_id, message.run_id.as_deref());
        {
            let mut runs = self.runs.lock().await;
            if runs.contains_key(thread_id) {
                return Err("Mobile chat is already running for this thread".into());
            }
            runs.insert(
                thread_id.to_string(),
                MobileRun {
                    run_id: run_id.clone(),
                    stop: Arc::new(AtomicBool::new(false)),
                    started_at: chrono::Utc::now().timestamp_millis(),
                },
            );
        }

        let manager = self.clone();
        let thread_id = thread_id.to_string();
        let app_handle = app_handle.clone();
        let title = message.conversation_title.clone();
        tokio::spawn(async move {
            manager
                .run_turn(&thread_id, &run_id, content, title, config, &app_handle)
                .await;
        });
        Ok(String::new())
    }

    async fn run_turn(
        &self,
        thread_id: &str,
        run_id: &str,
        content: String,
        title: Option<String>,
        config: MobileProviderConfig,
        app_handle: &tauri::AppHandle,
    ) {
        let emit = |chunk| {
            super::shared::emit_chunk_with_run_id(app_handle, &chunk, AGENT_TYPE, run_id);
        };

        // 1. 用户消息落库（thread_messages ── 云同步源）
        let user_message_id = format!("msg_{}", uuid::Uuid::now_v7().simple());
        if let Some(title) = title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let _ = self
                .thread_manager
                .ensure_thread(thread_id, AgentId::new(AGENT_TYPE), title.to_string())
                .await;
        }
        if let Err(error) = self
            .thread_manager
            .add_message(thread_id, Self::message(&user_message_id, "user", content.clone(), None))
            .await
        {
            tracing::error!("[mobile] failed to persist user message: {error}");
            emit(crate::agent_wire::AgentChunk::Error {
                thread_id: thread_id.to_string(),
                message: format!("MOBILE_PERSIST_FAILED: {error}"),
                error_details: None,
            });
            self.finish_run(app_handle, thread_id, run_id, None).await;
            return;
        }
        emit(crate::agent_wire::AgentChunk::UserMessage {
            thread_id: thread_id.to_string(),
            id: user_message_id,
            text: content.clone(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            attachments: Vec::new(),
        });
        emit(crate::agent_wire::AgentChunk::StreamStart {
            thread_id: thread_id.to_string(),
            model: Some(config.model.clone()),
            reasoning_effort: None,
        });

        // 2. 历史投影为 OpenAI 消息数组（reasoning/tool 只读字段不上行）
        let history = self
            .thread_manager
            .thread_payload_for_cloud(thread_id)
            .await
            .ok()
            .flatten()
            .map(|payload| {
                payload
                    .messages
                    .iter()
                    .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
                    .map(|message| {
                        serde_json::json!({
                            "role": message.role,
                            "content": message.content,
                        })
                    })
                    .collect::<Vec<Value>>()
            })
            .unwrap_or_default();

        let stop = self.stop_flag_for(thread_id).await;
        let mut assistant_content = String::new();
        let mut reasoning_content = String::new();
        let mut failed: Option<String> = None;

        match self
            .stream_completion(&config, history, &stop, app_handle, thread_id, run_id, &mut assistant_content, &mut reasoning_content)
            .await
        {
            Ok(()) => {}
            Err(error) => failed = Some(error),
        }

        // 3. 回合收尾：assistant 消息落库（停止/中断也保留已生成部分）
        let stopped = stop.load(Ordering::SeqCst);
        if !assistant_content.is_empty() {
            let final_content = if stopped {
                format!("{assistant_content}\n\n[已手动停止]")
            } else {
                assistant_content
            };
            let assistant_id = format!("msg_{}", uuid::Uuid::now_v7().simple());
            if let Err(error) = self
                .thread_manager
                .add_message(
                    thread_id,
                    Self::message(
                        &assistant_id,
                        "assistant",
                        final_content,
                        (!reasoning_content.is_empty()).then_some(reasoning_content.clone()),
                    ),
                )
                .await
            {
                tracing::error!("[mobile] failed to persist assistant message: {error}");
            }
            // 上云交给 A3 桥 pass 前对账（threads.updated_at 已随落库更新，
            // 下一轮账户级 Poll 30s 内推送，与移动端会话合并为同一 blob）。
        }

        let reason = if stopped {
            Some("user_stopped".to_string())
        } else {
            failed.clone()
        };
        if let Some(error) = failed {
            emit(crate::agent_wire::AgentChunk::Error {
                thread_id: thread_id.to_string(),
                message: error,
                error_details: None,
            });
        }
        self.finish_run(app_handle, thread_id, run_id, reason).await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn stream_completion(
        &self,
        config: &MobileProviderConfig,
        history: Vec<Value>,
        stop: &Arc<AtomicBool>,
        app_handle: &tauri::AppHandle,
        thread_id: &str,
        run_id: &str,
        assistant_content: &mut String,
        reasoning_content: &mut String,
    ) -> Result<(), String> {
        let body = serde_json::json!({
            "model": config.model,
            "messages": history,
            "stream": true,
        });
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|error| format!("MOBILE_CLIENT_BUILD_FAILED: {error}"))?;
        let response = client
            .post(config.chat_completions_url())
            .bearer_auth(&config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("MOBILE_REQUEST_FAILED: {error}"))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let detail = response.text().await.unwrap_or_default();
            return Err(format!(
                "MOBILE_HTTP_{status}: {}",
                detail.chars().take(300).collect::<String>()
            ));
        }

        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut finished = false;
        while !finished {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let chunk = tokio::time::timeout(
                std::time::Duration::from_secs(CHUNK_GAP_TIMEOUT_SECS),
                stream.next(),
            )
            .await;
            match chunk {
                Ok(Some(Ok(bytes))) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(newline) = buffer.find('\n') {
                        let line: String = buffer[..newline].trim_end_matches('\r').to_string();
                        buffer.drain(..=newline);
                        let Some(data) = line.strip_prefix("data:") else {
                            continue;
                        };
                        let data = data.trim();
                        if data == "[DONE]" {
                            finished = true;
                            break;
                        }
                        let Ok(event) = serde_json::from_str::<Value>(data) else {
                            continue;
                        };
                        let delta = &event["choices"][0]["delta"];
                        if let Some(reasoning) = delta["reasoning_content"].as_str() {
                            if !reasoning.is_empty() {
                                reasoning_content.push_str(reasoning);
                                super::shared::emit_chunk_with_run_id(
                                    app_handle,
                                    &crate::agent_wire::AgentChunk::Reasoning {
                                        thread_id: thread_id.to_string(),
                                        text: reasoning.to_string(),
                                    },
                                    AGENT_TYPE,
                                    run_id,
                                );
                            }
                        }
                        if let Some(text) = delta["content"].as_str() {
                            if !text.is_empty() {
                                assistant_content.push_str(text);
                                super::shared::emit_chunk_with_run_id(
                                    app_handle,
                                    &crate::agent_wire::AgentChunk::Text {
                                        thread_id: thread_id.to_string(),
                                        text: text.to_string(),
                                    },
                                    AGENT_TYPE,
                                    run_id,
                                );
                            }
                        }
                    }
                }
                Ok(Some(Err(error))) => {
                    // 网络中断：已有部分回答按已收内容收尾；否则报错
                    if assistant_content.is_empty() {
                        return Err(format!("MOBILE_STREAM_FAILED: {error}"));
                    }
                    tracing::warn!("[mobile] stream interrupted, keeping partial reply: {error}");
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    if assistant_content.is_empty() {
                        return Err("MOBILE_STREAM_TIMEOUT".into());
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    async fn stop_flag_for(&self, thread_id: &str) -> Arc<AtomicBool> {
        let runs = self.runs.lock().await;
        runs.get(thread_id)
            .map(|run| run.stop.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
    }

    async fn finish_run(
        &self,
        app_handle: &tauri::AppHandle,
        thread_id: &str,
        run_id: &str,
        reason: Option<String>,
    ) {
        {
            let mut runs = self.runs.lock().await;
            runs.remove(thread_id);
        }
        super::shared::emit_chunk_with_run_id(
            app_handle,
            &crate::agent_wire::AgentChunk::StreamEnd {
                thread_id: thread_id.to_string(),
                reason,
                duration_ms: None,
            },
            AGENT_TYPE,
            run_id,
        );
    }

    pub async fn stop_chat(&self, thread_id: &str, _run_id: Option<&str>) -> bool {
        let runs = self.runs.lock().await;
        if let Some(run) = runs.get(thread_id) {
            run.stop.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub async fn running_threads(&self) -> HashMap<String, RunInfo> {
        let runs = self.runs.lock().await;
        runs.iter()
            .map(|(thread_id, run)| {
                (
                    thread_id.clone(),
                    RunInfo {
                        started_at: run.started_at,
                        phase: Some("streaming".into()),
                        current_tool: None,
                        agent_type: Some(AGENT_TYPE.into()),
                        run_id: Some(run.run_id.clone()),
                        pending_thread_id: None,
                        session_id: None,
                    },
                )
            })
            .collect()
    }

    pub async fn stop_all(&self) -> usize {
        let runs = self.runs.lock().await;
        let count = runs.len();
        for run in runs.values() {
            run.stop.store(true, Ordering::SeqCst);
        }
        count
    }

    pub async fn reap_inactive_runs(
        &self,
        _app_handle: &tauri::AppHandle,
        idle_timeout_ms: i64,
    ) -> usize {
        // SSE 回合自带 120s chunk 间隔超时兜底；这里只按启动时长清理
        // 极端挂死（远超正常回合）的 run。
        let now = chrono::Utc::now().timestamp_millis();
        let mut runs = self.runs.lock().await;
        let stale: Vec<String> = runs
            .iter()
            .filter(|(_, run)| now - run.started_at > idle_timeout_ms.max(60_000))
            .map(|(thread_id, _)| thread_id.clone())
            .collect();
        for thread_id in &stale {
            if let Some(run) = runs.get(thread_id) {
                run.stop.store(true, Ordering::SeqCst);
            }
        }
        stale.len()
    }
}

#[async_trait]
impl ExternalCliRuntime for Arc<MobileChatManager> {
    fn kind(&self) -> ExternalRuntimeKind {
        ExternalRuntimeKind::Mobile
    }

    async fn chat_stream(
        &self,
        thread_id: &str,
        message: AgentUserMessage,
        app_handle: &tauri::AppHandle,
    ) -> Result<String, String> {
        MobileChatManager::chat_stream(self, thread_id, message, app_handle).await
    }

    async fn stop_chat(
        &self,
        thread_id: &str,
        run_id: Option<&str>,
        _app_handle: &tauri::AppHandle,
    ) -> bool {
        self.as_ref().stop_chat(thread_id, run_id).await
    }

    async fn running_threads(&self) -> HashMap<String, RunInfo> {
        self.as_ref().running_threads().await
    }

    async fn stop_all(&self) -> usize {
        self.as_ref().stop_all().await
    }

    async fn reap_inactive_runs(
        &self,
        app_handle: &tauri::AppHandle,
        idle_timeout_ms: i64,
    ) -> usize {
        self.as_ref()
            .reap_inactive_runs(app_handle, idle_timeout_ms)
            .await
    }
}
