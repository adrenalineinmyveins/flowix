//! F5 入站收件（FD-10 M3a）：从平台中继拉取咨询信封，校验后留档。
//!
//! 与出站 outbox 对称：入站信封**不进**任何业务存储，只落
//! `.flowix/redaction-inbound/`（契约 §2）；留档 status 驱动 UI 的
//! 接受（进 F4 编排）/ 拒绝。拉取语义 at-least-once，按 receiptId 去重。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use flowix_relay::envelope::{validate_structure, RejectReason, WireEnvelope};

pub const INBOUND_DIR_NAME: &str = "redaction-inbound";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboundStatus {
    /// 已拉取留档，等待用户处理。
    Pending,
    /// 用户接受（M3c：进 F4 编排）。
    Accepted,
    /// 用户拒绝或入站校验不通过。
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundRecord {
    pub receipt_id: String,
    /// 中继队列项 ID（ack 用；留档里仅作溯源）。
    pub queue_id: String,
    pub from_agent_id: String,
    pub from_display_name: String,
    pub to_agent_id: String,
    /// `consultation` | `reply`
    pub payload_kind: String,
    pub payload_text: String,
    /// 入站校验失败时的拒绝码（status=Rejected 时有值）。
    #[serde(default)]
    pub reject_code: Option<String>,
    /// 回程信封指向的原 receiptId。
    #[serde(default)]
    pub in_reply_to: Option<String>,
    pub disclosure: flowix_relay::envelope::WireDisclosure,
    pub received_at: i64,
    pub status: InboundStatus,
    /// FD-11 v2 验签结果（§5：结果入留档）：
    /// `verified`（信任锚公钥验签通过）/ `invalid`（验签失败）/
    /// `unsigned`（无签名块）/ `unverified`（from 无目录卡，缺信任锚）。
    #[serde(default)]
    pub signature_status: Option<String>,
}

pub fn inbound_dir(notebook_root: &Path) -> PathBuf {
    notebook_root.join(".flowix").join(INBOUND_DIR_NAME)
}

fn record_path(dir: &Path, receipt_id: &str) -> PathBuf {
    dir.join(format!("{receipt_id}.json"))
}

/// 规范化中继端点：relayUrl 兼容「base」「base/」「base/v1」「base/v1/」四种写法。
pub fn relay_endpoint(relay_url: &str, path: &str) -> String {
    let mut base = relay_url.trim().trim_end_matches('/').to_string();
    if base.ends_with("/v1") {
        base.truncate(base.len() - "/v1".len());
    }
    format!("{base}{path}")
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 入站校验：结构契约 + 收件人匹配（RECIPIENT_UNKNOWN 语义在收端=错投）。
pub fn validate_inbound(envelope: &WireEnvelope, local_agent_id: &str) -> Result<(), RejectReason> {
    validate_structure(envelope, now_unix())?;
    if envelope.to.agent_id != local_agent_id {
        return Err(RejectReason::RecipientUnknown);
    }
    Ok(())
}

/// 校验通过的入站信封 → 留档（Pending）。receiptId 已存在则跳过（去重幂等）。
/// `signature_status`：验签结论随档（见 InboundRecord 注释）。
pub fn store_inbound(
    dir: &Path,
    queue_id: &str,
    envelope: &WireEnvelope,
    signature_status: Option<String>,
) -> std::io::Result<bool> {
    std::fs::create_dir_all(dir)?;
    let path = record_path(dir, &envelope.provenance.receipt_id);
    if path.exists() {
        return Ok(false);
    }
    let record = InboundRecord {
        receipt_id: envelope.provenance.receipt_id.clone(),
        queue_id: queue_id.to_string(),
        from_agent_id: envelope.from.agent_id.clone(),
        from_display_name: envelope.from.display_name.clone(),
        to_agent_id: envelope.to.agent_id.clone(),
        payload_kind: envelope.payload.kind.clone(),
        payload_text: envelope.payload.text.clone(),
        reject_code: None,
        in_reply_to: envelope.in_reply_to.clone(),
        disclosure: envelope.disclosure.clone(),
        received_at: now_unix(),
        status: InboundStatus::Pending,
        signature_status,
    };
    let tmp = dir.join(format!("{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&record).expect("inbound serialize"))?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

/// 校验失败的入站信封 → 留档（Rejected + 拒绝码），同样按 receiptId 去重。
pub fn store_rejected(
    dir: &Path,
    queue_id: &str,
    envelope: &WireEnvelope,
    reason: RejectReason,
) -> std::io::Result<bool> {
    std::fs::create_dir_all(dir)?;
    let path = record_path(dir, &envelope.provenance.receipt_id);
    if path.exists() {
        return Ok(false);
    }
    let record = InboundRecord {
        receipt_id: envelope.provenance.receipt_id.clone(),
        queue_id: queue_id.to_string(),
        from_agent_id: envelope.from.agent_id.clone(),
        from_display_name: envelope.from.display_name.clone(),
        to_agent_id: envelope.to.agent_id.clone(),
        payload_kind: envelope.payload.kind.clone(),
        payload_text: envelope.payload.text.clone(),
        reject_code: Some(reason.code().to_string()),
        in_reply_to: envelope.in_reply_to.clone(),
        disclosure: envelope.disclosure.clone(),
        received_at: now_unix(),
        status: InboundStatus::Rejected,
        signature_status: None,
    };
    let tmp = dir.join(format!("{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&record).expect("inbound serialize"))?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

pub fn list_inbound(dir: &Path) -> Vec<InboundRecord> {
    let mut records: Vec<InboundRecord> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            std::fs::read(entry.path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
        })
        .collect();
    records.sort_by(|a, b| b.received_at.cmp(&a.received_at).then(a.receipt_id.cmp(&b.receipt_id)));
    records
}

pub fn set_inbound_status(dir: &Path, receipt_id: &str, status: InboundStatus) -> Result<(), String> {
    let path = record_path(dir, receipt_id);
    let bytes = std::fs::read(&path).map_err(|_| format!("INBOUND_RECORD_NOT_FOUND:{receipt_id}"))?;
    let mut record: InboundRecord = serde_json::from_slice(&bytes)
        .map_err(|e| format!("INBOUND_RECORD_INVALID:{e}"))?;
    if record.status != InboundStatus::Pending {
        return Err(format!("INBOUND_ALREADY_SETTLED:{receipt_id}"));
    }
    record.status = status;
    let tmp = dir.join(format!("{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&record).expect("inbound serialize"))
        .map_err(|e| format!("INBOUND_WRITE_FAILED:{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("INBOUND_WRITE_FAILED:{e}"))?;
    Ok(())
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollSummary {
    pub stored: usize,
    pub duplicates: usize,
    /// 入站校验拒绝并留档的数量。
    pub rejected: usize,
}

/// 拉取并留档一个 agent 的收件队列（HTTP 层薄封装，联调走真机 M4）。
/// 每个信封无论接受与否都会 ack（拒收也出队，理由在留档）。
pub async fn poll_agent_inbox(
    notebook_root: &Path,
    relay_url: &str,
    device_token: &str,
    agent_id: &str,
) -> Result<PollSummary, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("INBOX_CLIENT_INIT_FAILED:{e}"))?;
    let inbox_url = relay_endpoint(relay_url, "/v1/inbox");
    let response = client
        .get(inbox_url)
        .query(&[("agentId", agent_id)])
        .bearer_auth(device_token)
        .send()
        .await
        .map_err(|e| format!("INBOX_FETCH_FAILED:{e}"))?;
    if !response.status().is_success() {
        return Err(format!("INBOX_FETCH_REJECTED:{}", response.status().as_u16()));
    }
    let body: InboxResponseDto = response
        .json()
        .await
        .map_err(|e| format!("INBOX_DECODE_FAILED:{e}"))?;

    let dir = inbound_dir(notebook_root);
    let mut summary = PollSummary::default();
    let mut queue_ids = Vec::new();
    // 信任锚缓存：同一次拉取里同 from 只查一次目录卡。
    let mut public_keys: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for item in &body.envelopes {
        queue_ids.push(item.queue_id.clone());
        let signature_status = signature_status_of(&client, relay_url, &item.envelope, &mut public_keys).await;
        match validate_inbound(&item.envelope, agent_id) {
            Ok(()) => match store_inbound(&dir, &item.queue_id, &item.envelope, signature_status) {
                Ok(true) => summary.stored += 1,
                Ok(false) => summary.duplicates += 1,
                Err(e) => return Err(format!("INBOUND_STORE_FAILED:{e}")),
            },
            Err(reason) => match store_rejected(&dir, &item.queue_id, &item.envelope, reason) {
                Ok(true) => summary.rejected += 1,
                Ok(false) => summary.duplicates += 1,
                Err(e) => return Err(format!("INBOUND_STORE_FAILED:{e}")),
            },
        }
    }
    if !queue_ids.is_empty() {
        let ack_url = relay_endpoint(relay_url, "/v1/inbox/ack");
        let _ = client
            .post(ack_url)
            .bearer_auth(device_token)
            .json(&AckRequestDto { agent_id: agent_id.to_string(), queue_ids })
            .send()
            .await
            .map_err(|e| format!("INBOX_ACK_FAILED:{e}"))?;
    }
    Ok(summary)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboxResponseDto {
    envelopes: Vec<QueueItemDto>,
}

/// FD-11 §5 收端验签：无签名块 = `unsigned`；有签名但 from 无目录卡 =
/// `unverified`（缺信任锚，不拒收）；验签通过/失败 = `verified` / `invalid`
/// （结论入留档，由 UI 呈现；拒收决策留给人审）。
async fn signature_status_of(
    client: &reqwest::Client,
    relay_url: &str,
    envelope: &WireEnvelope,
    cache: &mut std::collections::HashMap<String, Option<String>>,
) -> Option<String> {
    if envelope.signature.is_none() {
        return Some("unsigned".to_string());
    }
    let from = envelope.from.agent_id.clone();
    if !cache.contains_key(&from) {
        let public_key = match from.split_once('/') {
            // from 即 `user/agent` 两段；裸 id（v1 匿名发起方）无目录可查。
            Some((user, slug)) => {
                let card_url = relay_endpoint(relay_url, &format!("/v2/directory/{user}/{slug}"));
                fetch_card_public_key(client, card_url).await
            }
            None => None,
        };
        cache.insert(from.clone(), public_key);
    }
    match cache.get(&from) {
        None => None,
        Some(None) => Some("unverified".to_string()),
        Some(Some(public_key)) => {
            let verified = super::account::verify_signature(envelope, public_key);
            Some(if verified { "verified" } else { "invalid" }.to_string())
        }
    }
}

async fn fetch_card_public_key(client: &reqwest::Client, card_url: String) -> Option<String> {
    let response = client.get(card_url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    body.get("card")?
        .get("publicKey")?
        .as_str()
        .map(str::to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueItemDto {
    queue_id: String,
    envelope: WireEnvelope,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AckRequestDto {
    agent_id: String,
    queue_ids: Vec<String>,
}

/// 向中继注册本机 agent 收件通道，返回 deviceToken。
/// FD-11 v2：有账号 token 时服务端把裸 slug 归一到 `username/slug` 命名空间。
pub async fn register_device(
    relay_url: &str,
    agent_ids: &[String],
    account_token: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("INBOX_CLIENT_INIT_FAILED: {e}"))?;
    let url = relay_endpoint(relay_url, "/v1/devices");
    let mut request = client.post(url).json(&serde_json::json!({ "agentIds": agent_ids }));
    if let Some(token) = account_token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("DEVICE_REGISTER_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("DEVICE_REGISTER_REJECTED:{}", response.status().as_u16()));
    }
    let body: DeviceTokenDto = response
        .json()
        .await
        .map_err(|e| format!("DEVICE_REGISTER_DECODE_FAILED: {e}"))?;
    Ok(body.device_token)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceTokenDto {
    device_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowix_relay::envelope::{AgentRef, WirePayload, WireProvenance, ENVELOPE_VERSION};

    fn sample(to_agent: &str) -> WireEnvelope {
        WireEnvelope {
            envelope: ENVELOPE_VERSION.to_string(),
            from: AgentRef { agent_id: "agt_a".into(), display_name: "A".into() },
            to: AgentRef { agent_id: to_agent.into(), display_name: "B".into() },
            in_reply_to: None,
            disclosure: flowix_relay::envelope::WireDisclosure {
                redaction_level: "R2".into(),
                max_hops: 2,
                hops_remaining: 2,
                expires_at: 4_102_444_800,
                allowed_scopes: vec![],
            },
            payload: WirePayload { kind: "consultation".into(), text: "远端问题".into() },
            provenance: WireProvenance {
                pipeline_version: "1".into(),
                rule_set_version: "rs".into(),
                used_fallback: false,
                confirmed_by: "user".into(),
                confirmed_at: 1,
                receipt_id: "rct_in1".into(),
            },
            signature: None,
        }
    }

    #[test]
    fn relay_endpoint_normalization() {
        assert_eq!(relay_endpoint("http://x:1", "/v1/inbox"), "http://x:1/v1/inbox");
        assert_eq!(relay_endpoint("http://x:1/", "/v1/inbox"), "http://x:1/v1/inbox");
        assert_eq!(relay_endpoint("http://x:1/v1", "/v1/inbox"), "http://x:1/v1/inbox");
        assert_eq!(relay_endpoint("http://x:1/v1/", "/v1/outbox"), "http://x:1/v1/outbox");
    }

    #[test]
    fn stores_pending_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = sample("agt_b");
        assert!(store_inbound(dir.path(), "q_1", &envelope, Some("unsigned".into())).unwrap());
        // 同 receipt 再拉到：去重。
        assert!(!store_inbound(dir.path(), "q_2", &envelope, None).unwrap());
        let records = list_inbound(dir.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, InboundStatus::Pending);
        assert_eq!(records[0].from_agent_id, "agt_a");
        assert_eq!(records[0].payload_text, "远端问题");
        assert_eq!(records[0].signature_status.as_deref(), Some("unsigned"));
    }

    #[test]
    fn wrong_recipient_is_rejected_and_archived() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = sample("agt_other");
        let reason = validate_inbound(&envelope, "agt_b").unwrap_err();
        assert_eq!(reason, RejectReason::RecipientUnknown);
        assert!(store_rejected(dir.path(), "q_1", &envelope, reason).unwrap());
        let records = list_inbound(dir.path());
        assert_eq!(records[0].status, InboundStatus::Rejected);
        assert_eq!(records[0].reject_code.as_deref(), Some("RECIPIENT_UNKNOWN"));
    }

    #[test]
    fn status_transitions_only_from_pending() {
        let dir = tempfile::tempdir().unwrap();
        let envelope = sample("agt_b");
        store_inbound(dir.path(), "q_1", &envelope, None).unwrap();
        set_inbound_status(dir.path(), "rct_in1", InboundStatus::Accepted).unwrap();
        // 已结算的不能再改。
        assert!(set_inbound_status(dir.path(), "rct_in1", InboundStatus::Rejected)
            .unwrap_err()
            .starts_with("INBOUND_ALREADY_SETTLED"));
        assert!(set_inbound_status(dir.path(), "rct_none", InboundStatus::Accepted)
            .unwrap_err()
            .starts_with("INBOUND_RECORD_NOT_FOUND"));
    }
}
