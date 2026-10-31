//! F4 咨询编排器：把 Pending 咨询经 DSH 主机跑成完整周期。
//!
//! 调用链（全部复用 deepseek_harness 的 stdio JSON-RPC 主机）：
//! `thread/start`（A 的咨询线程）→ `thread/subagent/spawn`（B 为子线程，
//! 天然受 maxDepth=4 / maxChildren=16 约束）→ 轮询
//! `flowix/session/usage`（预算）与 `thread/subagent/wait`（完成）→
//! `session/history` 取最终回答 → `thread/subagent/close` 清理。
//! 失败与预算熔断均落元数据与信箱；回答提取与用量解析为纯函数可单测。

use std::path::Path;

use serde_json::{json, Value};
use tauri::AppHandle;
use tauri::Manager;

use super::{
    append_consult_log, push_event, write_consult_meta, BudgetGuard, ConsultEvent, ConsultMeta,
    ConsultStatus,
};

const POLL_INTERVAL_MS: u64 = 2000;
const CONSULT_TIMEOUT_SECS: u64 = 600;
/// 单个 JSON-RPC 请求的兜底超时：DSH 对 inactive continuable 子代理的
/// 部分调用（如 session/history 的冷恢复）可能无限挂起，不带超时会让
/// 轮询循环整体卡死（2026-10-31 实机：meta 停在首轮 usage 后 5min 不动）。
const REQUEST_TIMEOUT_SECS: u64 = 20;

async fn timed_request(
    host: &dyn crate::agent_external::deepseek_harness::DshClient,
    value: Value,
) -> Result<Value, String> {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    match tokio::time::timeout(
        std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS),
        host.request(value),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(method, "orchestrator request timeout");
            Err(format!("request_timeout:{REQUEST_TIMEOUT_SECS}s"))
        }
    }
}

/// 兜底通道：直接解压子代理会话日志提取最终回答。JSON-RPC 查询
/// （usage/history）对 continuable 子代理会挂起且毒化后续请求
/// （2026-10-31 实机），而 DSH 落盘日志始终实时更新——答案以日志为准。
/// 日志形态：`<sessions>/<munged-cwd>/<child_id>/session.v4.jsonl.zstd`，
/// munged 目录名不反推（含空格转义等规则），在 sessions 下逐目录探测。
pub fn read_child_answer_from_session_log(
    sessions_dir: &Path,
    child_id: &str,
) -> Option<String> {
    let entries = std::fs::read_dir(sessions_dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let log_path = entry.path().join(child_id).join("session.v4.jsonl.zstd");
        if !log_path.is_file() {
            continue;
        }
        let text = decode_zstd_log(&log_path)?;
        if let Some(answer) = extract_last_assistant_from_log(&text) {
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if newest.as_ref().is_none_or(|(prev, _)| modified > *prev) {
                newest = Some((modified, answer));
            }
        }
    }
    newest.map(|(_, answer)| answer)
}

fn decode_zstd_log(path: &Path) -> Option<String> {
    use std::io::Write as _;
    let mut file = std::fs::File::open(path).ok()?;
    let mut decoder = ruzstd::FrameDecoder::new();
    let mut raw = Vec::new();
    // DSH 会话日志是多帧追加流（每批事件一个独立 zstd 帧，实机单文件
    // 可达 10+ 帧）。ruzstd 的 StreamingDecoder 单帧即止——当前帧排空后
    // 直接报 EOF，会静默丢掉后续帧（最终 assistant 回答恰在尾部帧）。
    // 必须逐帧 init + 全量解码 + 排空直到流尾。
    while decoder.init(&mut file).is_ok() {
        let mut failed = false;
        while !decoder.is_finished() {
            if decoder
                .decode_blocks(&mut file, ruzstd::BlockDecodingStrategy::All)
                .is_err()
            {
                // 尾帧可能仍在写入（半包）：保留已解出部分。
                failed = true;
                break;
            }
        }
        if failed || decoder.collect_to_writer(&mut raw).is_err() {
            break;
        }
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// 从会话日志 JSONL 提取最后一条 assistant 纯文本回答（跳过 tool-call）。
fn extract_last_assistant_from_log(log: &str) -> Option<String> {
    let mut last: Option<String> = None;
    for line in log.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant/message") {
            continue;
        }
        let content = value
            .pointer("/data/message/content")
            .and_then(Value::as_array)?;
        let text: Vec<String> = content
            .iter()
            .filter(|item| {
                item.get("type").and_then(Value::as_str) == Some("text")
            })
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let joined = text.join("");
        if !joined.trim().is_empty() {
            last = Some(joined.trim().to_string());
        }
    }
    last
}

/// 从 `session/history` 响应中提取最后一条 assistant 文本。
/// 响应结构（DSH native-dsh-runtime `sessionHistory`）：
/// `{sessionId, messages: [{id, role, content, messageType?…}], …}`。
/// `messageType` 标记非对话时间线投影（tool-call / dsh-command /
/// context-compaction 等），纯文本 assistant 消息不带。
pub fn extract_last_assistant(history: &Value) -> Option<String> {
    let items = history.get("messages").and_then(Value::as_array)?;
    items
        .iter()
        .rev()
        .filter_map(|item| {
            if item.get("messageType").and_then(Value::as_str).is_some() {
                return None;
            }
            let role = item.get("role").and_then(Value::as_str)?;
            let content = item.get("content").and_then(Value::as_str)?;
            Some((role, content))
        })
        .find(|(role, content)| *role == "assistant" && !content.trim().is_empty())
        .map(|(_, content)| content.trim().to_string())
}

/// 宽容解析 `flowix/session/usage`：兼容 tokensIn/tokensOut 与
/// inputTokens/outputTokens 两种命名，缺字段按 0 计。
pub fn parse_usage(usage: &Value) -> (u64, u64) {
    let pick = |names: &[&str]| -> u64 {
        names
            .iter()
            .find_map(|name| usage.get(*name).and_then(Value::as_u64))
            .unwrap_or(0)
    };
    (
        pick(&["tokensIn", "inputTokens", "input_tokens"]),
        pick(&["tokensOut", "outputTokens", "output_tokens"]),
    )
}

/// 检测最后回答中的反问标记（C-M3 文本协议）。
/// 返回标记内的问题文本；无标记返回 None。
pub fn extract_askback(answer: &str) -> Option<String> {
    let start = answer.find("<consult-askback>")?;
    let end = answer.rfind("</consult-askback>")?;
    if end <= start {
        return None;
    }
    let question = &answer[start + "<consult-askback>".len()..end];
    let trimmed = question.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

const MAX_ASKBACKS: usize = 3;
const ASKBACK_TIMEOUT_SECS: u64 = 120;

/// 消费一个 Pending 咨询。由 `consult_initiate` spawn 的 task 调用；
/// 任何失败路径都把 meta 置 Failed 并推信箱事件（水位式：不重试）。
pub async fn orchestrate(app: &AppHandle, notebook_root: &Path, mut meta: ConsultMeta, prompt: String) {
    let finish = |meta: &ConsultMeta, status: ConsultStatus| {
        let mut updated = meta.clone();
        updated.status = status;
        updated.updated_at = chrono::Utc::now().timestamp_millis();
        write_consult_meta(notebook_root, &updated);
        updated
    };
    let fail = |meta: &ConsultMeta, reason: &str| {
        let updated = finish(meta, ConsultStatus::Failed);
        push_event(&updated.consult_id, ConsultEvent::Failed { reason: reason.into() });
        append_consult_log(notebook_root, &updated, &ConsultEvent::Failed { reason: reason.into() });
        updated
    };

    let Some(state) = app.try_state::<crate::app::state::AppState>() else {
        fail(&meta, "app state unavailable");
        return;
    };
    let host = match state.deepseek_harness.model_host().await {
        Ok(host) => host,
        Err(error) => {
            fail(&meta, &format!("dsh_unavailable: {error}"));
            return;
        }
    };
    // thread/start 必须带 provider/model/preset/permission（裸启动会话 turn
    // 立即报 `agent has no provider/model`，子代理空转到超时）。
    let launch = match state.deepseek_harness.default_app_launch().await {
        Ok(launch) => launch,
        Err(error) => {
            fail(&meta, &format!("launch_config_unavailable: {error}"));
            return;
        }
    };

    let result = run_consult_cycle(
        &*host,
        &launch,
        &mut meta,
        notebook_root,
        state.user_config.dsh_sessions_dir(),
        &prompt,
    )
    .await;
    match result {
        Ok(answer) => {
            // F1 记账：专家被咨询 +1（本地 state；无 state 文件则新建）。
            {
                let mut expert_state = crate::commands::agent_profile::read_state_or_new(
                    notebook_root,
                    &meta.to_agent_id,
                );
                expert_state.consultations_received =
                    expert_state.consultations_received.saturating_add(1);
                expert_state.last_consulted_at = Some(chrono::Utc::now().timestamp_millis());
                let _ = crate::commands::agent_profile::write_state(notebook_root, &expert_state);
            }
            let updated = finish(&meta, ConsultStatus::Completed);
            let event = ConsultEvent::Result { answer, artifact_suggestion: None };
            push_event(&updated.consult_id, event.clone());
            append_consult_log(notebook_root, &updated, &event);
        }
        Err((reason, budget_exceeded)) => {
            let status = if budget_exceeded {
                ConsultStatus::BudgetExceeded
            } else {
                ConsultStatus::Failed
            };
            let updated = finish(&meta, status);
            let event = ConsultEvent::Failed { reason };
            push_event(&updated.consult_id, event.clone());
            append_consult_log(notebook_root, &updated, &event);
        }
    }
}

type CycleError = (String, bool);

#[allow(clippy::too_many_arguments)]
async fn run_consult_cycle(
    host: &dyn crate::agent_external::deepseek_harness::DshClient,
    launch: &(String, String, &'static str, &'static str),
    meta: &mut ConsultMeta,
    notebook_root: &Path,
    sessions_dir: std::path::PathBuf,
    prompt: &str,
) -> Result<String, CycleError> {
    let err = |reason: String| -> CycleError { (reason, false) };

    // 1. A 的咨询线程。
    let start = timed_request(
        host,
        json!({
            "jsonrpc": "2.0", "id": host.next_request_id(),
            "method": "thread/start",
            "params": {
                "flowixThreadId": meta.consult_id,
                "cwd": notebook_root.to_string_lossy(),
                "provider": launch.0, "model": launch.1,
                "agentPreset": launch.2, "permissionMode": launch.3
            }
        }),
    )
    .await
    .map_err(|e| err(format!("parent_start_failed: {e}")))?;
    // DSH App Server 的 thread/start 响应字段是 `thread.id`（与
    // deepseek_harness::manager 同口径；顶层 threadId 是旧协议残留）。
    let parent_id = start
        .pointer("/thread/id")
        .or_else(|| start.get("threadId"))
        .or_else(|| start.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| err("parent_start_failed: no threadId".into()))?
        .to_string();

    // 2. B 为子线程（depth 约束由 subagent-service 硬校验）。
    let spawn = timed_request(
        host,
        json!({
            "jsonrpc": "2.0", "id": host.next_request_id(),
            "method": "thread/subagent/spawn",
            "params": { "parentThreadId": parent_id, "prompt": prompt }
        }),
    )
    .await
    .map_err(|e| err(format!("spawn_failed: {e}")))?;
    let child_id = spawn
        .pointer("/subagent/childThreadId")
        .and_then(Value::as_str)
        .or_else(|| spawn.get("childThreadId").and_then(Value::as_str))
        .ok_or_else(|| err("spawn_failed: no childThreadId".into()))?
        .to_string();

    meta.status = ConsultStatus::Running;
    meta.child_thread_id = Some(child_id.clone());
    meta.updated_at = chrono::Utc::now().timestamp_millis();
    write_consult_meta(notebook_root, meta);

    // 3. 轮询：预算 + 完成 + 反问。
    let mut guard = BudgetGuard::new(meta.token_budget);
    let mut askbacks = 0usize;
    let mut last_answer: Option<String> = None;
    let mut tick: u32 = 0;
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(CONSULT_TIMEOUT_SECS);
    loop {
        // 心跳：定位轮询卡点（2026-10-31 实机：meta 停在 spawn 后不再前进）。
        tracing::info!(consult_id = %meta.consult_id, tick, "consult poll tick");
        if tokio::time::Instant::now() >= deadline {
            let _ = interrupt_and_close(host, &child_id, &parent_id).await;
            return Err(("run_failed: consult timed out".into(), false));
        }
        // 预算。
        if let Ok(usage) = timed_request(
            host,
            json!({
                "jsonrpc": "2.0", "id": host.next_request_id(),
                "method": "flowix/session/usage",
                "params": { "sessionId": child_id }
            }),
        )
        .await
        {
            let (tokens_in, tokens_out) = parse_usage(&usage);
            guard.record(tokens_in.saturating_add(tokens_out));
            meta.tokens_used = guard.used;
            write_consult_meta(notebook_root, meta);
            push_event(
                &meta.consult_id,
                ConsultEvent::BudgetUsed { tokens: guard.used, remaining: guard.remaining() },
            );
            if guard.should_cancel() {
                let _ = interrupt_and_close(host, &child_id, &parent_id).await;
                return Err(("budget_exceeded".into(), true));
            }
        }
        // 完成（或反问）。native continuable 子代理没有 host 级 run
        // completion——上游 nativeStatus 只产 running|idle，TERMINAL 判定
        // 永假，subagent/wait 的 completed 恒 false（2026-10-31 实机：子代理
        // turn 3s completed，wait 轮询 5min 仍超时）。改用 history 双采样
        // 稳定判定：连续两轮提取到相同非空 assistant 文本即视为完成。
        let history = timed_request(
            host,
            json!({
                "jsonrpc": "2.0", "id": host.next_request_id(),
                "method": "session/history",
                "params": { "sessionId": child_id, "limit": 50 }
            }),
        )
        .await;
        // JSON-RPC history 挂起/失败时兜底读会话日志（日志始终实时落盘）。
        let current_answer = history
            .ok()
            .as_ref()
            .and_then(extract_last_assistant)
            .or_else(|| read_child_answer_from_session_log(&sessions_dir, &child_id));
        let stable = match (&current_answer, &last_answer) {
            (Some(current), Some(previous)) => current == previous,
            _ => false,
        };
        if stable {
            let answer = current_answer.expect("stable implies Some");
            let asks_back = extract_askback(&answer);
            if let (Some(question), true) = (asks_back, askbacks < MAX_ASKBACKS) {
                askbacks += 1;
                if handle_askback(host, meta, notebook_root, &child_id, question).await {
                    // 答复已注入，回到轮询等 B 继续。
                    last_answer = None;
                    continue;
                }
                // 等待通道异常（如 cancel 已移除 waiter）：按未答复兜底注入。
                let _ = send_followup(
                    host,
                    &child_id,
                    "咨询方未答复，请基于现有信息作答。",
                )
                .await;
                last_answer = None;
                continue;
            }
            break;
        }
        last_answer = current_answer;
        tick += 1;
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }

    // 4. 取最终回答（反问轮次之后的输出）。history 挂起/失败时回退用
    // 稳定判定轮已提取的答案（两者同源，稳定轮即最终文本）。
    let answer = match timed_request(
        host,
        json!({
            "jsonrpc": "2.0", "id": host.next_request_id(),
            "method": "session/history",
            "params": { "sessionId": child_id, "limit": 50 }
        }),
    )
    .await
    {
        Ok(history) => extract_last_assistant(&history)
            .ok_or_else(|| ("run_failed: no assistant output".to_string(), false))?,
        Err(_) => last_answer
            .or_else(|| read_child_answer_from_session_log(&sessions_dir, &child_id))
            .ok_or_else(|| ("run_failed: history unavailable".to_string(), false))?,
    };

    // 5. 清理。
    let _ = interrupt_and_close(host, &child_id, &parent_id).await;
    Ok(answer)
}

/// 处理一次反问：置 WaitingAskback、推信箱事件、注册等待通道并阻塞等
/// 答复（120s），收到则经 `thread/subagent/send`（followupTask 模式）注入
/// B 的线程并恢复 Running。返回 false 表示等待通道异常（无答复可注入）。
async fn handle_askback(
    host: &dyn crate::agent_external::deepseek_harness::DshClient,
    meta: &mut ConsultMeta,
    notebook_root: &Path,
    child_id: &str,
    question: String,
) -> bool {
    meta.status = ConsultStatus::WaitingAskback;
    meta.updated_at = chrono::Utc::now().timestamp_millis();
    write_consult_meta(notebook_root, meta);
    push_event(
        &meta.consult_id,
        ConsultEvent::Askback { question: question.clone(), options: Vec::new() },
    );
    append_consult_log(
        notebook_root,
        meta,
        &ConsultEvent::Askback { question, options: Vec::new() },
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    super::register_askback_waiter(&meta.consult_id, tx);
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(ASKBACK_TIMEOUT_SECS),
        rx.recv(),
    )
    .await;
    let answer = match answer {
        Ok(Some(answer)) => answer,
        // 超时：注入未答复提示，让 B 基于现有信息继续。
        _ => "咨询方未答复，请基于现有信息作答。".to_string(),
    };
    let sent = send_followup(host, child_id, &answer).await;
    meta.status = ConsultStatus::Running;
    meta.updated_at = chrono::Utc::now().timestamp_millis();
    write_consult_meta(notebook_root, meta);
    sent
}

/// 经 `thread/subagent/send` 以 followupTask 模式向 B 注入一条消息。
async fn send_followup(
    host: &dyn crate::agent_external::deepseek_harness::DshClient,
    child_id: &str,
    input: &str,
) -> bool {
    host.request(json!({
        "jsonrpc": "2.0", "id": host.next_request_id(),
        "method": "thread/subagent/send",
        "params": { "childThreadId": child_id, "input": input, "mode": "followupTask" }
    }))
    .await
    .is_ok()
}

async fn interrupt_and_close(
    host: &dyn crate::agent_external::deepseek_harness::DshClient,
    child_id: &str,
    parent_id: &str,
) {
    // close 全部走超时兜底：nativeClose 的 drain 在 continuable 子代理上
    // 可能无限挂起，且会把 host 的 JSON-RPC 处理整体卡死，毒化后续所有
    // 编排请求（2026-10-31 实机：上轮编排的 close 挂死后，下一轮编排的
    // usage/history 全部无响应）。放弃清理无碍——child 留在 DSH 侧
    // continuable，不占本地资源。
    for (method, params) in [
        ("thread/subagent/close", json!({ "childThreadId": child_id })),
        ("thread/close", json!({ "threadId": parent_id })),
    ] {
        let _ = timed_request(
            host,
            json!({
                "jsonrpc": "2.0", "id": host.next_request_id(),
                "method": method, "params": params
            }),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_last_assistant_message() {
        let history = json!({
            "sessionId": "session-x",
            "messages": [
                { "role": "user", "content": "问题" },
                { "role": "assistant", "content": "" },
                // 工具调用投影（带 messageType）不算回答。
                { "role": "assistant", "messageType": "tool-call", "content": "{\"q\":1}" },
                { "role": "assistant", "content": "  最终回答  " }
            ]
        });
        assert_eq!(extract_last_assistant(&history).as_deref(), Some("最终回答"));
        assert!(extract_last_assistant(&json!({ "messages": [] })).is_none());
        assert!(extract_last_assistant(&json!({})).is_none());
    }

    #[test]
    fn parses_usage_across_field_names() {
        assert_eq!(
            parse_usage(&json!({ "tokensIn": 10, "tokensOut": 5 })),
            (10, 5)
        );
        assert_eq!(
            parse_usage(&json!({ "inputTokens": 7, "outputTokens": 3 })),
            (7, 3)
        );
        assert_eq!(parse_usage(&json!({})), (0, 0));
    }

    #[test]
    fn extracts_askback_marker() {
        assert_eq!(
            extract_askback("<consult-askback>需要 Q3 的物流时效数据</consult-askback>")
                .as_deref(),
            Some("需要 Q3 的物流时效数据")
        );
        // 前后有噪声文本仍可提取。
        assert_eq!(
            extract_askback("前置说明\n<consult-askback>  问题  </consult-askback>\n尾注")
                .as_deref(),
            Some("问题")
        );
        // 无标记 / 空问题 / 只有开头标签。
        assert!(extract_askback("普通回答").is_none());
        assert!(extract_askback("<consult-askback>   </consult-askback>").is_none());
        assert!(extract_askback("<consult-askback>没有闭合").is_none());
    }
}
