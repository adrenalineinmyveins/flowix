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

/// 从 `session/history` 响应中提取最后一条 assistant 文本。
pub fn extract_last_assistant(history: &Value) -> Option<String> {
    let items = history.get("history").and_then(Value::as_array)?;
    items
        .iter()
        .rev()
        .filter_map(|item| {
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

/// 取 `completed` 标志（thread/subagent/wait 响应）。
pub fn parse_wait_completed(wait: &Value) -> bool {
    wait.get("completed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

    let result = run_consult_cycle(&*host, &mut meta, notebook_root, &prompt).await;
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

async fn run_consult_cycle(
    host: &dyn crate::agent_external::deepseek_harness::DshClient,
    meta: &mut ConsultMeta,
    notebook_root: &Path,
    prompt: &str,
) -> Result<String, CycleError> {
    let err = |reason: String| -> CycleError { (reason, false) };

    // 1. A 的咨询线程。
    let start = host
        .request(json!({
            "jsonrpc": "2.0", "id": host.next_request_id(),
            "method": "thread/start",
            "params": { "flowixThreadId": meta.consult_id, "cwd": notebook_root.to_string_lossy() }
        }))
        .await
        .map_err(|e| err(format!("parent_start_failed: {e}")))?;
    let parent_id = start
        .get("threadId")
        .and_then(Value::as_str)
        .ok_or_else(|| err("parent_start_failed: no threadId".into()))?
        .to_string();

    // 2. B 为子线程（depth 约束由 subagent-service 硬校验）。
    let spawn = host
        .request(json!({
            "jsonrpc": "2.0", "id": host.next_request_id(),
            "method": "thread/subagent/spawn",
            "params": { "parentThreadId": parent_id, "prompt": prompt }
        }))
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
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(CONSULT_TIMEOUT_SECS);
    loop {
        if tokio::time::Instant::now() >= deadline {
            let _ = interrupt_and_close(host, &child_id, &parent_id).await;
            return Err(("run_failed: consult timed out".into(), false));
        }
        // 预算。
        if let Ok(usage) = host
            .request(json!({
                "jsonrpc": "2.0", "id": host.next_request_id(),
                "method": "flowix/session/usage",
                "params": { "sessionId": child_id }
            }))
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
        // 完成（或反问）。
        let completed = host
            .request(json!({
                "jsonrpc": "2.0", "id": host.next_request_id(),
                "method": "thread/subagent/wait",
                "params": { "childThreadId": child_id }
            }))
            .await
            .ok()
            .and_then(|wait| parse_wait_completed(&wait).then_some(wait));
        if let Some(_wait) = completed {
            let history = host
                .request(json!({
                    "jsonrpc": "2.0", "id": host.next_request_id(),
                    "method": "session/history",
                    "params": { "sessionId": child_id, "limit": 50 }
                }))
                .await
                .map_err(|e| ("history_failed".to_string(), false))?;
            let asks_back = extract_last_assistant(&history)
                .and_then(|text| extract_askback(&text));
            if let (Some(question), true) = (asks_back, askbacks < MAX_ASKBACKS) {
                askbacks += 1;
                if handle_askback(host, meta, notebook_root, &child_id, question).await {
                    // 答复已注入，回到轮询等 B 继续。
                    continue;
                }
                // 等待通道异常（如 cancel 已移除 waiter）：按未答复兜底注入。
                let _ = send_followup(
                    host,
                    &child_id,
                    "咨询方未答复，请基于现有信息作答。",
                )
                .await;
                continue;
            }
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }

    // 4. 取最终回答（反问轮次之后的输出）。
    let history = host
        .request(json!({
            "jsonrpc": "2.0", "id": host.next_request_id(),
            "method": "session/history",
            "params": { "sessionId": child_id, "limit": 50 }
        }))
        .await
        .map_err(|e| (format!("history_failed: {e}"), false))?;
    let answer = extract_last_assistant(&history)
        .ok_or_else(|| ("run_failed: no assistant output".to_string(), false))?;

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
    for (method, params) in [
        ("thread/subagent/close", json!({ "childThreadId": child_id })),
        ("thread/close", json!({ "threadId": parent_id })),
    ] {
        let _ = host
            .request(json!({
                "jsonrpc": "2.0", "id": host.next_request_id(),
                "method": method, "params": params
            }))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_last_assistant_message() {
        let history = json!({
            "history": [
                { "role": "user", "content": "问题" },
                { "role": "assistant", "content": "" },
                { "role": "assistant", "content": "  最终回答  " }
            ]
        });
        assert_eq!(extract_last_assistant(&history).as_deref(), Some("最终回答"));
        assert!(extract_last_assistant(&json!({ "history": [] })).is_none());
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
    fn parses_wait_completed_flag() {
        assert!(parse_wait_completed(&json!({ "completed": true })));
        assert!(!parse_wait_completed(&json!({ "completed": false })));
        assert!(!parse_wait_completed(&json!({})));
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
