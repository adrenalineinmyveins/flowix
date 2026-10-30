//! 移动端 Agent 网络接入（MOBILE-AGENT-NETWORK M3）：网络身份注册、
//! 收件箱前台轮询、consult 注入会话流。协议实现复用 flowix-relay-client
//! （M2 自桌面 redaction 抽取，双端同源）；私钥与令牌仅存 app 私有目录，
//! 不参与云同步（R7）。
//!
//! - 身份：Ed25519 密钥即身份（account.json）+ 收件通道令牌（device.json）
//! - agent 命名：固定 slug `mob` → agent_id = `{username}/mob`
//! - consult 注入：入站信封（经 at-least-once 拉取 + receiptId 去重）以
//!   user 消息落入网络会话流 `th_net_inbox`，origin="network"

use std::path::{Path, PathBuf};

use flowix_relay_client::account::{read_account_at, register_or_login_at, RelayAccount};
use flowix_relay_client::inbound::{
    list_inbound, poll_agent_inbox, register_device, InboundRecord, PollSummary,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::agent::{AgentState, THREADS_CHANGED_EVENT};

/// 网络会话流（单一收件会话；多条 consult 顺序追加）。
pub(crate) const NET_THREAD_ID: &str = "th_net_inbox";
/// 移动端固定 agent slug（桌面订阅/consult 对此地址寻址）。
pub(crate) const NET_AGENT_SLUG: &str = "mob";

/// 收件通道凭据（/v1/devices 发放，与 account.json 同目录）。
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCredential {
    agent_id: String,
    device_token: String,
}

/// 中继存储根（app 私有目录下 redaction/）。
fn root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("redaction"))
        .map_err(|e| e.to_string())
}

fn read_device(root: &Path) -> Option<DeviceCredential> {
    std::fs::read_to_string(root.join("device.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn write_device(root: &Path, credential: &DeviceCredential) -> Result<(), String> {
    let raw = serde_json::to_string(credential).map_err(|e| e.to_string())?;
    flowix_core::memo_file::atomic_write_bytes(&root.join("device.json"), raw.as_bytes())
        .map_err(|e| e.to_string())
}

fn credentials(app: &AppHandle) -> Result<Option<(RelayAccount, DeviceCredential)>, String> {
    let root = root(app)?;
    Ok(read_account_at(&root).zip(read_device(&root)))
}

/// 已注册账号的中继地址（市场等只读消费方使用；未注册返回 None）。
pub(crate) fn account_relay_url(app: &AppHandle) -> Option<String> {
    let root = root(app).ok()?;
    read_account_at(&root).map(|account| account.relay_url)
}

/// consult → 网络会话流消息（纯函数，可单测）。
pub(crate) fn consult_message(record: &InboundRecord) -> flowix_sync::V2ThreadMessage {
    let kind_label = match record.payload_kind.as_str() {
        "consultation" => "咨询",
        "reply" => "回信",
        other => other,
    };
    flowix_sync::V2ThreadMessage {
        id: format!("net_{}", record.receipt_id),
        role: "user".into(),
        content: format!(
            "【{kind_label}】来自 {}（{}）：\n{}",
            record.from_display_name, record.from_agent_id, record.payload_text
        ),
        reasoning: None,
        tool_name: Some("consult".into()),
        tool_input: None,
        tool_data: Some(serde_json::json!({
            "receiptId": record.receipt_id,
            "inReplyTo": record.in_reply_to,
            "fromAgentId": record.from_agent_id,
            "fromDisplayName": record.from_display_name,
            "kind": record.payload_kind,
            "encrypted": record.encrypted,
        })
        .to_string()),
        created_at: record.received_at * 1000,
        origin: Some("network".into()),
        attachments: Vec::new(),
    }
}

/// 新入站记录注入网络会话流（幂等：消息 id = `net_{receiptId}`，
/// 既有 id 在 merge 路径下并集去重）。
fn inject_consult(
    app: &AppHandle,
    agent: &std::sync::Arc<AgentState>,
    sync: &std::sync::Arc<flowix_sync::SyncManager>,
    record: &InboundRecord,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut payload = agent.thread_payload(NET_THREAD_ID)?.unwrap_or_else(|| {
        flowix_sync::V2ThreadPayload {
            version: 1,
            thread_id: NET_THREAD_ID.into(),
            agent_id: "network".into(),
            title: "网络咨询".into(),
            created_at: now,
            messages: Vec::new(),
            starred: false,
            archived: false,
        }
    });
    let message = consult_message(record);
    let message_id = message.id.clone();
    if payload.messages.iter().any(|m| m.id == message_id) {
        return Ok(());
    }
    payload.messages.push(message);
    agent.save_thread_payload(&payload)?;
    crate::agent::enqueue_thread_change_and_sync(app, agent, sync, &payload);
    Ok(())
}

/// 拉取收件箱并注入新信封（命令与轮询任务共用入口）。
pub(crate) async fn poll_and_inject(app: AppHandle) -> Result<PollSummary, String> {
    let Some((account, device)) = credentials(&app)? else {
        return Err("redaction_not_registered".into());
    };
    let archive_root = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let summary = poll_agent_inbox(
        &archive_root,
        &account.relay_url,
        &device.device_token,
        &device.agent_id,
    )
    .await?;
    if !summary.stored_receipt_ids.is_empty() {
        let records = list_inbound(&flowix_relay_client::inbound::inbound_dir(&archive_root));
        let agent_state = app.state::<std::sync::Arc<AgentState>>();
        let sync = app.state::<std::sync::Arc<flowix_sync::SyncManager>>();
        for receipt_id in &summary.stored_receipt_ids {
            if let Some(record) = records.iter().find(|r| &r.receipt_id == receipt_id) {
                if let Err(error) =
                    inject_consult(&app, &agent_state, &sync, record)
                {
                    // 注入失败不阻断整趟（留档仍在，下次拉取不重发——
                    // receiptId 已去重；可手动「拉取收件箱」重试注入路径）
                    let _ = app.emit(
                        "redaction-inbound-error",
                        format!("CONSULT_INJECT_FAILED: {error}"),
                    );
                }
            }
        }
        let _ = app.emit(THREADS_CHANGED_EVENT, ());
    }
    Ok(summary)
}

/// 前台轮询任务：30s 间隔；未注册静默跳过，失败静默（下次重试）。
pub(crate) fn spawn_poll_task(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if credentials(&app).ok().flatten().is_some() {
                let _ = poll_and_inject(app.clone()).await;
            }
        }
    });
}

// ── IPC 命令 ──────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionStatus {
    pub registered: bool,
    pub username: String,
    pub relay_url: String,
    pub public_key: String,
    pub agent_id: String,
}

#[tauri::command]
pub fn redaction_status(app: AppHandle) -> Result<RedactionStatus, String> {
    match credentials(&app)? {
        Some((account, device)) => Ok(RedactionStatus {
            registered: true,
            username: account.username.clone(),
            relay_url: account.relay_url.clone(),
            public_key: account.public_key.clone(),
            agent_id: device.agent_id,
        }),
        None => Ok(RedactionStatus {
            registered: false,
            username: String::new(),
            relay_url: String::new(),
            public_key: String::new(),
            agent_id: String::new(),
        }),
    }
}

/// 注册/登录中继账号并开通本机收件通道（username 已存在走挑战登录）。
#[tauri::command]
pub async fn redaction_register(
    app: AppHandle,
    username: String,
    relay_url: String,
) -> Result<Value, String> {
    let username = username.trim().to_string();
    if username.is_empty() || username.contains('/') {
        return Err("redaction_register_rejected: 用户名不能为空或包含 /".into());
    }
    flowix_relay_client::transport::validate_relay_url(&relay_url)
        .map_err(|e| format!("redaction_register_rejected: {e}"))?;
    let root = root(&app)?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let account = register_or_login_at(&root, &username, &relay_url)
        .await
        .map_err(|e| {
            let lower = e.to_lowercase();
            if lower.contains("connect")
                || lower.contains("timeout")
                || lower.contains("dns")
                || lower.contains("unreachable")
            {
                format!("redaction_relay_unreachable: {e}")
            } else {
                format!("redaction_register_rejected: {e}")
            }
        })?;
    let agent_id = format!("{}/{}", account.username, NET_AGENT_SLUG);
    let device_token = register_device(&relay_url, &[agent_id.clone()], Some(&account.device_token))
        .await
        .map_err(|e| format!("redaction_register_rejected: {e}"))?;
    write_device(&root, &DeviceCredential {
        agent_id: agent_id.clone(),
        device_token,
    })?;
    let mut info = serde_json::to_value(account.public_info()).map_err(|e| e.to_string())?;
    info["agentId"] = Value::String(agent_id);
    Ok(info)
}

/// 手动拉取一次收件箱（轮询任务之外的前台触发口）。
#[tauri::command]
pub async fn redaction_poll_once(app: AppHandle) -> Result<Value, String> {
    let summary = poll_and_inject(app).await?;
    Ok(serde_json::json!({
        "received": summary.stored,
        "duplicates": summary.duplicates,
        "rejected": summary.rejected,
    }))
}

/// 忘记本机网络身份（删除私钥与令牌；云端账号仍在，可重新登录恢复）。
#[tauri::command]
pub fn redaction_forget(app: AppHandle) -> Result<(), String> {
    let root = root(&app)?;
    for name in ["account.json", "device.json"] {
        let path = root.join(name);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// M4 人工代答回信：对网络会话流里最近一条 consult 出站回信。
/// 回信文本 = consult 之后最后一条 assistant 消息（AI 起草，用户点
/// 「发送回信」即确认）。出站复用桌面同源 outbox 确认管线：签名 →
/// 留档（Confirmed）→ 单一出口投递；对端公钥未知时为签名明文
/// （E2E 盲转依赖目录卡公钥预取，见设计开放项 M4b）。
#[tauri::command]
pub async fn redaction_reply(app: AppHandle, thread_id: String) -> Result<Value, String> {
    let Some((account, device)) = credentials(&app)? else {
        return Err("redaction_not_registered".into());
    };
    let agent_state = app.state::<std::sync::Arc<AgentState>>();
    let sync = app.state::<std::sync::Arc<flowix_sync::SyncManager>>();
    let Some(mut payload) = agent_state.thread_payload(&thread_id)? else {
        return Err("redaction_no_pending_consult".into());
    };
    let consult_index = payload
        .messages
        .iter()
        .rposition(|message| {
            message.role == "user"
                && message.origin.as_deref() == Some("network")
                && message.tool_name.as_deref() == Some("consult")
        })
        .ok_or_else(|| "redaction_no_pending_consult".to_string())?;
    let consult = &payload.messages[consult_index];
    let meta: Value = serde_json::from_str(consult.tool_data.as_deref().unwrap_or("{}"))
        .map_err(|e| format!("redaction_no_pending_consult: {e}"))?;
    let in_reply_to = meta["receiptId"]
        .as_str()
        .ok_or_else(|| "redaction_no_pending_consult: receiptId 缺失".to_string())?
        .to_string();
    let to_agent_id = meta["fromAgentId"].as_str().unwrap_or_default().to_string();
    let to_display_name = meta["fromDisplayName"].as_str().unwrap_or_default().to_string();
    if to_agent_id.is_empty() {
        return Err("redaction_no_pending_consult: fromAgentId 缺失".into());
    }
    // AI 起草文本：consult 之后最后一条 assistant 消息
    let draft = payload.messages[consult_index + 1..]
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| message.content.trim().to_string())
        .unwrap_or_default();
    if draft.is_empty() {
        return Err("redaction_no_draft: 请先与 AI 对话起草回信，再点发送".into());
    }

    let archive_root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut record = flowix_relay_client::outbox::RedactionOutboxRecord {
        receipt_id: flowix_relay_client::outbox::new_receipt_id(),
        status: flowix_relay_client::outbox::OutboxStatus::Draft,
        created_at: now_ms,
        settled_at: None,
        question: consult.content.chars().take(80).collect(),
        scope_entry_ids: Vec::new(),
        allowed_scopes: Vec::new(),
        target_level: "r1".into(),
        rule_set_version: "mobile-m4".into(),
        payload: draft.clone(),
        placeholder_map: std::collections::BTreeMap::new(),
        l1_hits: Vec::new(),
        included_paths: Vec::new(),
        dropped_paths: Vec::new(),
        attempts: 0,
        used_fallback: false,
        truncated: false,
        from_agent: flowix_relay_client::outbox::AgentParty {
            agent_id: device.agent_id.clone(),
            display_name: account.username.clone(),
        },
        to_agent: flowix_relay_client::outbox::AgentParty {
            agent_id: to_agent_id.clone(),
            display_name: to_display_name.clone(),
        },
        in_reply_to: None,
        reply_hops_remaining: None,
        signature: None,
        blind_relay: false,
        recipient_public_key: None,
        cipher_text: None,
        used_auto_gate: None,
        distill_mode: None,
    }
    .with_reply(in_reply_to.clone(), flowix_relay_client::outbox::DISCLOSURE_MAX_HOPS - 1);
    let signature = account
        .sign_envelope(&record.envelope())
        .map_err(|e| format!("redaction_deliver_failed: 签名失败 {e}"))?;
    record.signature = Some(signature);
    flowix_relay_client::outbox::write_record(&archive_root, &record)?;
    flowix_relay_client::outbox::settle(
        &archive_root,
        &record.receipt_id,
        flowix_relay_client::outbox::OutboxStatus::Confirmed,
    )?;
    let result =
        flowix_relay_client::transport::deliver_confirmed_envelope_with_token(
            &archive_root,
            &record.receipt_id,
            &account.relay_url,
            Some(&account.device_token),
        )
        .await
        .map_err(|e| format!("redaction_deliver_failed: {e}"))?;

    // 回执落网络会话流（幂等 id）
    let reply_message = flowix_sync::V2ThreadMessage {
        id: format!("net_reply_{}", record.receipt_id),
        role: "assistant".into(),
        content: format!("已回信 {to_agent_id}：\n{draft}"),
        reasoning: None,
        tool_name: Some("consult".into()),
        tool_input: None,
        tool_data: Some(
            serde_json::json!({
                "replyReceiptId": record.receipt_id,
                "inReplyTo": in_reply_to,
                "toAgentId": to_agent_id,
                "httpStatus": result.http_status,
            })
            .to_string(),
        ),
        created_at: chrono::Utc::now().timestamp_millis(),
        origin: Some("network".into()),
        attachments: Vec::new(),
    };
    let reply_id = reply_message.id.clone();
    if payload.messages.iter().any(|m| m.id == reply_id) {
        return Ok(serde_json::json!({ "delivered": true, "duplicate": true }));
    }
    payload.messages.push(reply_message);
    agent_state.save_thread_payload(&payload)?;
    crate::agent::enqueue_thread_change_and_sync(&app, &agent_state, &sync, &payload);
    let _ = app.emit(THREADS_CHANGED_EVENT, ());
    Ok(serde_json::json!({
        "delivered": true,
        "httpStatus": result.http_status,
        "byteLength": result.byte_length,
        "replyReceiptId": record.receipt_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: &str, receipt: &str) -> InboundRecord {
        InboundRecord {
            receipt_id: receipt.into(),
            queue_id: "q1".into(),
            from_agent_id: "alice/agent".into(),
            from_display_name: "Alice".into(),
            to_agent_id: "bob/mob".into(),
            payload_kind: kind.into(),
            payload_text: "帮我查一下 X".into(),
            reject_code: None,
            in_reply_to: None,
            disclosure: serde_json::from_str(
                r#"{"redactionLevel":"R1","maxHops":1,"hopsRemaining":1,"expiresAt":1770003600}"#,
            )
            .expect("valid disclosure"),
            received_at: 1770000000,
            status: flowix_relay_client::inbound::InboundStatus::Pending,
            signature_status: None,
            encrypted: false,
        }
    }

    #[test]
    fn consult_message_carries_receipt_meta() {
        let message = consult_message(&record("consultation", "rcpt_1"));
        assert_eq!(message.id, "net_rcpt_1");
        assert_eq!(message.role, "user");
        assert_eq!(message.origin.as_deref(), Some("network"));
        assert!(message.content.contains("【咨询】"));
        assert!(message.content.contains("Alice"));
        assert!(message.content.contains("帮我查一下 X"));
        let meta: Value = serde_json::from_str(message.tool_data.as_deref().unwrap()).unwrap();
        assert_eq!(meta["receiptId"], "rcpt_1");
        assert_eq!(meta["fromAgentId"], "alice/agent");
    }
}
