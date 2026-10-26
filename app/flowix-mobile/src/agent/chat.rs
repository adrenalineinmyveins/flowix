//! OpenAI 兼容 SSE 直连引擎（方案 A4）。
//!
//! 协议对齐桌面 `agent-chunk`（`#[serde(tag = "kind")]` 内部 tag、字段
//! snake_case——`app.emit` 不走 IPC camelCase 转换，TS 侧手写镜像类型）。
//! 长连接客户端不设总超时（默认 30s timeout 会掐断流），靠 chunk 间隔
//! 与停止标志兜底。用户消息与回合收尾各自落库 → `record_v2_thread_change`
//! → 防抖同步（fingerprint 幂等）。

use super::{AgentState, THREADS_CHANGED_EVENT};
use flowix_sync::{V2ThreadMessage, V2ThreadPayload};
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// 流式事件（wire 协议，字段 snake_case）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentChunk {
    StreamStart { thread_id: String },
    Text { thread_id: String, delta: String },
    Reasoning { thread_id: String, delta: String },
    Error { thread_id: String, message: String },
    StreamEnd { thread_id: String },
}

/// 每线程停止标志（chat_stop 置位，流循环检查）。
static ACTIVE_STOP_FLAGS: Mutex<Option<HashMap<String, Arc<AtomicBool>>>> = Mutex::new(None);

fn stop_flag(thread_id: &str) -> Arc<AtomicBool> {
    let mut guard = ACTIVE_STOP_FLAGS.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .entry(thread_id.to_string())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

pub(crate) fn request_stop(thread_id: &str) {
    let mut guard = ACTIVE_STOP_FLAGS.lock().unwrap();
    if let Some(flags) = guard.as_mut() {
        if let Some(flag) = flags.get_mut(thread_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

/// 笔记上下文（P1a）：卡片发送时携带所在笔记，首条消息注入 [CONTEXT] 块。
/// 与桌面 store/context-block.ts 同款格式；落库 content 仍存用户原文，
/// 上下文只在请求瞬间拼接（等价桌面 content/llmContent 分离）。
#[derive(Debug, Clone)]
pub(crate) struct ChatNoteContext {
    pub notebook_id: String,
    pub relative_path: String,
}

pub(crate) const CONTEXT_PROMPT_MARKER: &str = "<## CONTEXT PROMPT ##>";
const CONTEXT_NOTE_PREVIEW_CHARS: usize = 500;

/// 读笔记并构造上下文块（读失败静默降级为无上下文，不阻断发送）。
fn build_note_context_suffix(
    state: &crate::cache::MobileState,
    context: &ChatNoteContext,
) -> Option<String> {
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&context.notebook_id)?;
    let path = crate::cache::path_from_relative(
        std::path::Path::new(&notebook.path),
        &context.relative_path,
    )
    .ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let preview: String = content
        .replace("\r\n", "\n")
        .trim()
        .chars()
        .take(CONTEXT_NOTE_PREVIEW_CHARS)
        .collect();
    if preview.is_empty() {
        return None;
    }
    Some(format!(
        "{CONTEXT_PROMPT_MARKER}\n\n当前笔记内容（前{CONTEXT_NOTE_PREVIEW_CHARS}字）:\n{preview}"
    ))
}

/// 发送一条消息并流式收回答。立即返回；进度经 `agent-chunk` 事件推送。
/// 前端不信本函数的返回值判定流结束——以 `stream_end` chunk 为准。
pub(crate) async fn chat_send(
    app: AppHandle,
    agent: Arc<AgentState>,
    sync: Arc<flowix_sync::SyncManager>,
    state: &crate::cache::MobileState,
    thread_id: Option<String>,
    content: String,
    note_context: Option<ChatNoteContext>,
) -> Result<String, String> {
    let config = agent.provider_config()?;
    if !config.is_ready() {
        return Err("AGENT_PROVIDER_NOT_CONFIGURED".into());
    }
    let thread_id = match thread_id {
        Some(id) if id.starts_with("th_") => id,
        _ => AgentState::new_thread_id(),
    };
    if stop_flag(&thread_id).load(Ordering::SeqCst) {
        return Err("AGENT_CHAT_ALREADY_STOPPED".into());
    }

    // 1. 落用户消息（幂等合并路径：本地已有则并集）
    let now = chrono::Utc::now().timestamp_millis();
    let mut payload = agent
        .thread_payload(&thread_id)?
        .unwrap_or_else(|| V2ThreadPayload {
            version: 1,
            thread_id: thread_id.clone(),
            agent_id: "mobile".into(),
            title: String::new(),
            created_at: now,
            messages: Vec::new(),
            starred: false,
            archived: false,
        });
    // 上下文块只随首条消息注入（对齐桌面 isFirstMessage 语义），
    // 且只进请求不落库——落库 content 恒为用户原文。
    let llm_context_suffix = if payload.messages.is_empty() {
        note_context.and_then(|context| build_note_context_suffix(state, &context))
    } else {
        None
    };
    payload.messages.push(V2ThreadMessage {
        id: AgentState::new_message_id(),
        role: "user".into(),
        content: content.clone(),
        reasoning: None,
        tool_name: None,
        tool_input: None,
        tool_data: None,
        created_at: now,
        origin: Some("mobile".into()),
    });
    // 首条消息自动生成标题（与桌面体验一致）
    if payload.title.trim().is_empty() {
        let title: String = content.chars().take(30).collect();
        payload.title = title.trim().to_string();
    }
    agent.save_thread_payload(&payload)?;
    let _ = app.emit(THREADS_CHANGED_EVENT, ());
    // 用户消息落库即入队上云：回合失败（请求/流错误）时收尾路径不会
    // enqueue，否则该消息会滞留本地直到该线程下一个成功回合。
    // fingerprint 覆盖整个 payload，回合收尾再次 enqueue 是幂等更新。
    super::enqueue_thread_change_and_sync(&app, &agent, &sync, &payload);

    // 2. spawn 流式任务（不阻塞 IPC）
    tauri::async_runtime::spawn(async move {
        run_chat_turn(app, agent, sync, payload, config, llm_context_suffix).await;
    });
    Ok(thread_id)
}

async fn run_chat_turn(
    app: AppHandle,
    agent: Arc<AgentState>,
    sync: Arc<flowix_sync::SyncManager>,
    mut payload: V2ThreadPayload,
    config: super::AgentProviderConfig,
    llm_context_suffix: Option<String>,
) {
    let thread_id = payload.thread_id.clone();
    let stop = stop_flag(&thread_id);
    stop.store(false, Ordering::SeqCst);
    let _ = app.emit(
        "agent-chunk",
        AgentChunk::StreamStart {
            thread_id: thread_id.clone(),
        },
    );

    // 请求体：历史投影为 OpenAI 消息数组（reasoning/tool 只读字段不上行）。
    // 最后一条 user 消息按需拼接上下文块（llmContent 语义，落库仍为原文）。
    let last_user_index = payload
        .messages
        .iter()
        .rposition(|message| message.role == "user");
    let messages: Vec<Value> = payload
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| matches!(message.role.as_str(), "user" | "assistant"))
        .map(|(index, message)| {
            let mut llm_content = message.content.clone();
            if Some(index) == last_user_index {
                if let Some(suffix) = llm_context_suffix.as_ref() {
                    llm_content.push_str("\n");
                    llm_content.push_str(suffix);
                }
            }
            serde_json::json!({
                "role": message.role,
                "content": llm_content,
            })
        })
        .collect();
    let body = serde_json::json!({
        "model": config.model,
        "messages": messages,
        "stream": true,
    });

    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            emit_error_and_end(&app, &thread_id, &error.to_string());
            return;
        }
    };
    let request = client
        .post(config.chat_completions_url())
        .bearer_auth(&config.api_key)
        .json(&body);

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            emit_error_and_end(&app, &thread_id, &format!("AGENT_REQUEST_FAILED: {error}"));
            return;
        }
    };
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let detail = response.text().await.unwrap_or_default();
        emit_error_and_end(
            &app,
            &thread_id,
            &format!("AGENT_HTTP_{status}: {}", detail.chars().take(300).collect::<String>()),
        );
        return;
    }

    // 3. SSE 流消费：行缓冲 + data: 帧解析（无现成 SSE crate，手写）
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut assistant_content = String::new();
    let mut reasoning_content = String::new();
    let mut finished = false;
    while !finished {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(120), stream.next()).await;
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
                            let _ = app.emit(
                                "agent-chunk",
                                AgentChunk::Reasoning {
                                    thread_id: thread_id.clone(),
                                    delta: reasoning.to_string(),
                                },
                            );
                        }
                    }
                    if let Some(text) = delta["content"].as_str() {
                        if !text.is_empty() {
                            assistant_content.push_str(text);
                            let _ = app.emit(
                                "agent-chunk",
                                AgentChunk::Text {
                                    thread_id: thread_id.clone(),
                                    delta: text.to_string(),
                                },
                            );
                        }
                    }
                }
            }
            Ok(Some(Err(error))) => {
                if !assistant_content.is_empty() {
                    // 网络中断但已有部分回答：按已收内容收尾落库
                    eprintln!("[agent] stream interrupted, keeping partial reply: {error}");
                } else {
                    emit_error_and_end(&app, &thread_id, &format!("AGENT_STREAM_FAILED: {error}"));
                    return;
                }
                break;
            }
            Ok(None) => break,
            Err(_) => {
                // 120s 无任何 chunk：按超时收尾
                if assistant_content.is_empty() {
                    emit_error_and_end(&app, &thread_id, "AGENT_STREAM_TIMEOUT");
                    return;
                }
                break;
            }
        }
    }

    // 4. 回合收尾：assistant 消息落库（停止/中断也保留已生成部分）
    let stopped = stop.load(Ordering::SeqCst);
    stop.store(false, Ordering::SeqCst);
    if !assistant_content.is_empty() {
        let now = chrono::Utc::now().timestamp_millis();
        payload.messages.push(V2ThreadMessage {
            id: AgentState::new_message_id(),
            role: "assistant".into(),
            content: if stopped {
                format!("{assistant_content}\n\n[已手动停止]")
            } else {
                assistant_content
            },
            reasoning: (!reasoning_content.is_empty()).then_some(reasoning_content),
            tool_name: None,
            tool_input: None,
            tool_data: None,
            created_at: now,
            origin: Some("mobile".into()),
        });
        if let Err(error) = agent.save_thread_payload(&payload) {
            emit_error_and_end(&app, &thread_id, &format!("AGENT_PERSIST_FAILED: {error}"));
            return;
        }
        super::enqueue_thread_change_and_sync(&app, &agent, &sync, &payload);
    }
    let _ = app.emit(
        "agent-chunk",
        AgentChunk::StreamEnd {
            thread_id: thread_id.clone(),
        },
    );
    let _ = app.emit(THREADS_CHANGED_EVENT, ());
}

fn emit_error_and_end(app: &AppHandle, thread_id: &str, message: &str) {
    let _ = app.emit(
        "agent-chunk",
        AgentChunk::Error {
            thread_id: thread_id.to_string(),
            message: message.to_string(),
        },
    );
    let _ = app.emit(
        "agent-chunk",
        AgentChunk::StreamEnd {
            thread_id: thread_id.to_string(),
        },
    );
}
