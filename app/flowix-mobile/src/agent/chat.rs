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
    attachments: Vec<flowix_sync::V2MessageAttachment>,
) -> Result<String, String> {
    let config = agent.provider_config()?;
    if !config.is_ready() {
        return Err("AGENT_PROVIDER_NOT_CONFIGURED".into());
    }
    if content.trim().is_empty() && attachments.is_empty() {
        return Err("AGENT_CHAT_EMPTY".into());
    }
    let thread_id = match thread_id {
        Some(id) if id.starts_with("th_") => id,
        // 新会话页前端传空串（非 null）：视同未指定，走新建
        Some(id) if id.trim().is_empty() => AgentState::new_thread_id(),
        // 桌面同步卡携带桌面本地 id（无 th_ 前缀）：归一到已同步的 th_ 实体
        // 续聊；未同步到本机时报错，不静默新建分叉线程。
        Some(id) => match agent.thread_payload_normalized(&id)? {
            Some((normalized, _)) => normalized,
            None => return Err("AGENT_THREAD_NOT_SYNCED".into()),
        },
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
        attachments,
    });
    // 首条消息自动生成标题（与桌面体验一致；纯图消息用占位）
    if payload.title.trim().is_empty() {
        let title = if content.trim().is_empty() {
            "[图片]".to_string()
        } else {
            content.chars().take(30).collect()
        };
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

/// 单轮流式补全输出：正文/思考（已边收边 emit），以及完整累积的 tool_calls。
struct TurnOutput {
    content: String,
    reasoning: String,
    /// (id, name, arguments_json)
    tool_calls: Vec<(String, String, String)>,
}

/// 工具循环上限：防模型连续调用工具烧钱；用尽后按已有内容收尾。
const MAX_TOOL_ROUNDS: usize = 5;

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

    // 历史投影为 OpenAI 消息数组（reasoning/tool 历史不上行——旧工具结果
    // 不进上下文，本轮工具结果在循环内以标准 messages 传递）。
    let last_user_index = payload
        .messages
        .iter()
        .rposition(|message| message.role == "user");
    let mut messages: Vec<Value> = payload
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
            if message.role == "user" && !message.attachments.is_empty() {
                let mut parts = vec![serde_json::json!({ "type": "text", "text": llm_content })];
                for attachment in &message.attachments {
                    if let Some(url) = attachment_data_url(attachment) {
                        parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": { "url": url }
                        }));
                    }
                }
                if parts.len() > 1 {
                    return serde_json::json!({
                        "role": message.role,
                        "content": parts,
                    });
                }
            }
            serde_json::json!({
                "role": message.role,
                "content": llm_content,
            })
        })
        .collect();

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

    let web_search_tool = serde_json::json!({
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "联网搜索最新信息。当问题涉及时效性、新闻、价格、版本号或你不确定的事实性内容时调用。",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索关键词" }
                },
                "required": ["query"]
            }
        }
    });

    // 工具循环：模型请求工具 → 执行搜索 → 结果回填 → 再请求，直到纯文本回答或达上限。
    let mut assistant_content = String::new();
    let mut reasoning_content = String::new();
    for round in 0..MAX_TOOL_ROUNDS {
        let mut body = serde_json::json!({
            "model": config.model,
            "messages": &messages,
            "stream": true,
        });
        body["tools"] = serde_json::json!([web_search_tool]);
        let output = match stream_completion(
            &app,
            &thread_id,
            &stop,
            &client,
            &config,
            body,
        )
        .await
        {
            Ok(output) => output,
            Err(error) => {
                emit_error_and_end(&app, &thread_id, &error);
                return;
            }
        };
        reasoning_content.push_str(&output.reasoning);
        if output.tool_calls.is_empty() {
            assistant_content = output.content;
            break;
        }

        // 1) assistant(tool_calls) 消息入上下文（OpenAI 协议要求 tool 前置）
        let calls: Vec<Value> = output
            .tool_calls
            .iter()
            .map(|(id, name, args)| {
                serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect();
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": if output.content.is_empty() { Value::Null } else { serde_json::json!(output.content) },
            "tool_calls": calls,
        }));

        // 2) 执行每个工具调用：搜索 → 工具消息落库（UI 工具行）+ 上下文回填
        let mut round_failed = false;
        for (call_id, name, args_json) in &output.tool_calls {
            let query = serde_json::from_str::<Value>(args_json)
                .ok()
                .and_then(|args| args["query"].as_str().map(str::to_string))
                .unwrap_or_default();
            if name != "web_search" || query.trim().is_empty() {
                let hint = format!("AGENT_TOOL_UNSUPPORTED: {name}");
                messages.push(serde_json::json!({
                    "role": "tool", "tool_call_id": call_id, "content": hint,
                }));
                round_failed = true;
                continue;
            }
            let results = web_search_dispatch(&client, &config, &query).await;
            let (tool_data, llm_content) = match &results {
                Ok(data) => (data.clone(), data.clone()),
                Err(error) => {
                    let hint = format!("搜索失败：{error}");
                    round_failed = true;
                    (hint.clone(), hint)
                }
            };
            // 工具过程走思考通道（不污染正文气泡）
            let _ = app.emit(
                "agent-chunk",
                AgentChunk::Reasoning {
                    thread_id: thread_id.clone(),
                    delta: format!("\n[工具] web_search(\"{query}\")\n{}\n", tool_data),
                },
            );
            payload.messages.push(V2ThreadMessage {
                id: AgentState::new_message_id(),
                role: "tool".into(),
                content: String::new(),
                reasoning: None,
                tool_name: Some("web_search".into()),
                tool_input: Some(args_json.clone()),
                tool_data: Some(tool_data),
                created_at: chrono::Utc::now().timestamp_millis(),
                origin: Some("mobile".into()),
                attachments: Vec::new(),
            });
            messages.push(serde_json::json!({
                "role": "tool", "tool_call_id": call_id, "content": llm_content,
            }));
        }
        if let Err(error) = agent.save_thread_payload(&payload) {
            emit_error_and_end(&app, &thread_id, &format!("AGENT_PERSIST_FAILED: {error}"));
            return;
        }
        super::enqueue_thread_change_and_sync(&app, &agent, &sync, &payload);
        let _ = app.emit(THREADS_CHANGED_EVENT, ());
        if round_failed && round + 1 == MAX_TOOL_ROUNDS {
            break;
        }
    }

    // 回合收尾：assistant 消息落库（停止/中断也保留已生成部分）
    let stopped = stop.load(Ordering::SeqCst);
    stop.store(false, Ordering::SeqCst);
    if assistant_content.is_empty() && !stopped {
        assistant_content = "（工具调用达到上限，未能生成最终回答，请重试）".into();
    }
    if !assistant_content.is_empty() {
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
            created_at: chrono::Utc::now().timestamp_millis(),
            origin: Some("mobile".into()),
            attachments: Vec::new(),
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

/// 单轮 SSE 补全：text/reasoning 增量边收边 emit；tool_calls 增量按 index 累积。
/// 返回 Err 仅在「无任何可用内容」的失败场景（与旧单轮语义一致）。
async fn stream_completion(
    app: &AppHandle,
    thread_id: &str,
    stop: &Arc<std::sync::atomic::AtomicBool>,
    client: &reqwest::Client,
    config: &super::AgentProviderConfig,
    body: Value,
) -> Result<TurnOutput, String> {
    let request = client
        .post(config.chat_completions_url())
        .bearer_auth(&config.api_key)
        .json(&body);
    let response = request.send().await.map_err(|error| {
        format!("AGENT_REQUEST_FAILED: {error}")
    })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let detail = response.text().await.unwrap_or_default();
        return Err(format!(
            "AGENT_HTTP_{status}: {}",
            detail.chars().take(300).collect::<String>()
        ));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut output = TurnOutput {
        content: String::new(),
        reasoning: String::new(),
        tool_calls: Vec::new(),
    };
    // tool_calls 增量按 index 定位累积（OpenAI 流式协议）
    let mut call_slots: Vec<(String, String, String)> = Vec::new();
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
                    let choice = &event["choices"][0];
                    let delta = &choice["delta"];
                    if let Some(reasoning) = delta["reasoning_content"].as_str() {
                        if !reasoning.is_empty() {
                            output.reasoning.push_str(reasoning);
                            let _ = app.emit(
                                "agent-chunk",
                                AgentChunk::Reasoning {
                                    thread_id: thread_id.to_string(),
                                    delta: reasoning.to_string(),
                                },
                            );
                        }
                    }
                    if let Some(text) = delta["content"].as_str() {
                        if !text.is_empty() {
                            output.content.push_str(text);
                            let _ = app.emit(
                                "agent-chunk",
                                AgentChunk::Text {
                                    thread_id: thread_id.to_string(),
                                    delta: text.to_string(),
                                },
                            );
                        }
                    }
                    if let Some(calls) = delta["tool_calls"].as_array() {
                        for call in calls {
                            let index = call["index"].as_u64().unwrap_or(0) as usize;
                            while call_slots.len() <= index {
                                call_slots.push((String::new(), String::new(), String::new()));
                            }
                            let slot = &mut call_slots[index];
                            if let Some(id) = call["id"].as_str() {
                                if !id.is_empty() {
                                    slot.0 = id.to_string();
                                }
                            }
                            if let Some(name) = call["function"]["name"].as_str() {
                                if !name.is_empty() {
                                    slot.1 = name.to_string();
                                }
                            }
                            if let Some(args) = call["function"]["arguments"].as_str() {
                                slot.2.push_str(args);
                            }
                        }
                    }
                }
            }
            Ok(Some(Err(error))) => {
                if output.content.is_empty() && call_slots.is_empty() {
                    return Err(format!("AGENT_STREAM_FAILED: {error}"));
                }
                eprintln!("[agent] stream interrupted, keeping partial reply: {error}");
                break;
            }
            Ok(None) => break,
            Err(_) => {
                if output.content.is_empty() && call_slots.is_empty() {
                    return Err("AGENT_STREAM_TIMEOUT".into());
                }
                break;
            }
        }
    }
    output.tool_calls = call_slots
        .into_iter()
        .filter(|(_, name, _)| !name.is_empty())
        .collect();
    Ok(output)
}

// ── 联网搜索工具（方案 B）──────────────────────────────────────────
// Tavily（配置 key 时，结果质量高）或内置必应网页抓取（免 key 兜底）。

async fn web_search_dispatch(
    client: &reqwest::Client,
    config: &super::AgentProviderConfig,
    query: &str,
) -> Result<String, String> {
    match config.search_api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => tavily_search(client, key, query).await,
        None => bing_search(client, query).await,
    }
}

/// Tavily API：POST /search，返回 markdown 化的结果列表。
async fn tavily_search(client: &reqwest::Client, key: &str, query: &str) -> Result<String, String> {
    let response = client
        .post("https://api.tavily.com/search")
        .bearer_auth(key)
        .json(&serde_json::json!({ "query": query, "max_results": 5 }))
        .send()
        .await
        .map_err(|e| format!("TAVILY_REQUEST_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("TAVILY_HTTP_{}", response.status().as_u16()));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("TAVILY_PARSE_FAILED: {e}"))?;
    let mut lines = Vec::new();
    if let Some(results) = value["results"].as_array() {
        for (i, item) in results.iter().enumerate().take(5) {
            let title = item["title"].as_str().unwrap_or_default();
            let url = item["url"].as_str().unwrap_or_default();
            let snippet = item["content"].as_str().unwrap_or_default();
            lines.push(format!("{}. {} | {}\n{}", i + 1, title, url, snippet));
        }
    }
    if lines.is_empty() {
        return Err("TAVILY_EMPTY".into());
    }
    Ok(lines.join("\n\n"))
}

/// 必应网页抓取（免 key 兜底）：cn.bing.com 结果页粗解析（b_algo 块的
/// 标题/链接/摘要）。页面结构变化时会退化到空结果，模型将如实告知。
async fn bing_search(client: &reqwest::Client, query: &str) -> Result<String, String> {
    let url = format!(
        "https://cn.bing.com/search?q={}",
        urlencode(query)
    );
    let response = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124 Mobile Safari/537.36")
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .send()
        .await
        .map_err(|e| format!("BING_REQUEST_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("BING_HTTP_{}", response.status().as_u16()));
    }
    let html = response
        .text()
        .await
        .map_err(|e| format!("BING_READ_FAILED: {e}"))?;
    let mut lines = Vec::new();
    for block in html.split("<li class=\"b_algo").skip(1).take(5) {
        let Some(link) = capture_first(block, "href=\"", "\"") else {
            continue;
        };
        let Some(title) = capture_first(block, "<h2>", "</h2>") else {
            continue;
        };
        let snippet = capture_first(block, "<p", "</p>")
            .map(|raw| {
                // 去标签 + 反转义
                let text = strip_tags(&raw);
                html_unescape(&text)
            })
            .unwrap_or_default();
        let title = html_unescape(&strip_tags(&title));
        let snippet = snippet.chars().take(300).collect::<String>();
        if !link.starts_with("http") {
            continue;
        }
        lines.push(format!("{}. {} | {}\n{}", lines.len() + 1, title, link, snippet));
    }
    if lines.is_empty() {
        return Err("BING_EMPTY（页面结构可能已变化）".into());
    }
    Ok(lines.join("\n\n"))
}

/// 从 text 中取 start 之后、首个 end 之前的片段（片段内标签保留，调用方清洗）。
fn capture_first(text: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = text.find(start)? + start.len();
    let rest = &text[start_idx..];
    let end_idx = rest.find(end)?;
    Some(rest[..end_idx].to_string())
}

fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn html_unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// 附件 → data URL（读失败/空文件返回 None，调用方跳过该附件）。
/// mime 按文件名扩展推断，缺省 png。
fn attachment_data_url(attachment: &flowix_sync::V2MessageAttachment) -> Option<String> {
    let bytes = std::fs::read(&attachment.storage_key).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let mime = match attachment
        .file_name
        .rsplit('.')
        .next()
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    };
    use base64::Engine as _;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
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
