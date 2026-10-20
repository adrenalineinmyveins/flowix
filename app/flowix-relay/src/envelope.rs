//! 信封 wire 契约 v1（`flowix.consult.v1`）的类型与校验。
//! 契约文档：docs/F5-NETWORK-RELAY.md §3。

use serde::{Deserialize, Serialize};

pub const ENVELOPE_VERSION: &str = "flowix.consult.v1";
pub const PAYLOAD_MAX_BYTES: usize = 64 * 1024;

/// 中继与收端共用的拒绝码（契约 §3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    EnvelopeVersionUnsupported,
    DisclosureIncomplete,
    PayloadTooLarge,
    RecipientUnknown,
    Expired,
    HopsExhausted,
}

impl RejectReason {
    pub fn code(self) -> &'static str {
        match self {
            RejectReason::EnvelopeVersionUnsupported => "ENVELOPE_VERSION_UNSUPPORTED",
            RejectReason::DisclosureIncomplete => "DISCLOSURE_INCOMPLETE",
            RejectReason::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            RejectReason::RecipientUnknown => "RECIPIENT_UNKNOWN",
            RejectReason::Expired => "EXPIRED",
            RejectReason::HopsExhausted => "HOPS_EXHAUSTED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    pub agent_id: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireDisclosure {
    /// `R1|R2|R3`
    pub redaction_level: String,
    pub max_hops: u32,
    pub hops_remaining: u32,
    /// Unix 秒。
    pub expires_at: i64,
    /// `tag:…` / `path:…` 范围标签。
    #[serde(default)]
    pub allowed_scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePayload {
    /// `consultation` | `reply`
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireProvenance {
    pub pipeline_version: String,
    pub rule_set_version: String,
    #[serde(default)]
    pub used_fallback: bool,
    pub confirmed_by: String,
    pub confirmed_at: i64,
    pub receipt_id: String,
}

/// FD-11 v2 出站签名块（wire v1 兼容扩展：缺省不序列化，旧收端忽略）。
/// 签名范围 = `WireEnvelope`（signature 置 None）的确定性 JSON 序列化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireSignature {
    pub alg: String,
    /// 签名者公钥标识（目录卡公钥，base64；亦作验签信任锚）。
    pub key_id: String,
    /// base64(标准) 的 64 字节 Ed25519 签名。
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEnvelope {
    pub envelope: String,
    pub from: AgentRef,
    pub to: AgentRef,
    /// 回程信封必填；首发省略。
    #[serde(default)]
    pub in_reply_to: Option<String>,
    pub disclosure: WireDisclosure,
    pub payload: WirePayload,
    pub provenance: WireProvenance,
    /// FD-11 v2：可选出站签名；无签名信封按匿名档配额（中继执法）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<WireSignature>,
}

/// 信封规范化序列化（签名计算与验签的统一字节源）。
pub fn canonical_bytes(envelope: &WireEnvelope) -> Vec<u8> {
    let mut unsigned = envelope.clone();
    unsigned.signature = None;
    serde_json::to_vec(&unsigned).expect("envelope serializable")
}

/// 结构级校验（中继入口执行；`RecipientUnknown` 由 store 判定，见 routes）。
pub fn validate_structure(envelope: &WireEnvelope, now_unix: i64) -> Result<(), RejectReason> {
    if envelope.envelope != ENVELOPE_VERSION {
        return Err(RejectReason::EnvelopeVersionUnsupported);
    }
    let disclosure = &envelope.disclosure;
    if !matches!(disclosure.redaction_level.as_str(), "R1" | "R2" | "R3")
        || disclosure.max_hops == 0
        || disclosure.hops_remaining > disclosure.max_hops
    {
        return Err(RejectReason::DisclosureIncomplete);
    }
    if envelope.from.agent_id.is_empty() || envelope.to.agent_id.is_empty() {
        return Err(RejectReason::DisclosureIncomplete);
    }
    if !matches!(envelope.payload.kind.as_str(), "consultation" | "reply") {
        return Err(RejectReason::DisclosureIncomplete);
    }
    // 回程信封必须带 inReplyTo（回答不是首发）。
    if envelope.payload.kind == "reply" && envelope.in_reply_to.is_none() {
        return Err(RejectReason::DisclosureIncomplete);
    }
    if envelope.provenance.receipt_id.is_empty() {
        return Err(RejectReason::DisclosureIncomplete);
    }
    if envelope.payload.text.len() > PAYLOAD_MAX_BYTES {
        return Err(RejectReason::PayloadTooLarge);
    }
    if disclosure.expires_at <= now_unix {
        return Err(RejectReason::Expired);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> WireEnvelope {
        WireEnvelope {
            envelope: ENVELOPE_VERSION.to_string(),
            from: AgentRef { agent_id: "agt_a".into(), display_name: "A".into() },
            to: AgentRef { agent_id: "agt_b".into(), display_name: "B".into() },
            in_reply_to: None,
            disclosure: WireDisclosure {
                redaction_level: "R2".into(),
                max_hops: 2,
                hops_remaining: 2,
                expires_at: 4_102_444_800,
                allowed_scopes: vec!["tag:finance/财报".into()],
            },
            payload: WirePayload { kind: "consultation".into(), text: "问题…".into() },
            provenance: WireProvenance {
                pipeline_version: "1".into(),
                rule_set_version: "rs1".into(),
                used_fallback: false,
                confirmed_by: "user".into(),
                confirmed_at: 1,
                receipt_id: "rct_1".into(),
            },
            signature: None,
        }
    }

    #[test]
    fn accepts_valid_envelope() {
        assert_eq!(validate_structure(&sample(), 1), Ok(()));
    }

    #[test]
    fn rejects_wrong_version() {
        let mut e = sample();
        e.envelope = "flowix.consult.v0".into();
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::EnvelopeVersionUnsupported));
    }

    #[test]
    fn rejects_incomplete_disclosure() {
        let mut e = sample();
        e.disclosure.redaction_level = "R9".into();
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::DisclosureIncomplete));

        let mut e = sample();
        e.disclosure.hops_remaining = 5;
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::DisclosureIncomplete));

        let mut e = sample();
        e.to.agent_id = String::new();
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::DisclosureIncomplete));

        let mut e = sample();
        e.payload.kind = "chat".into();
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::DisclosureIncomplete));

        let mut e = sample();
        e.payload.kind = "reply".into();
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::DisclosureIncomplete));
    }

    #[test]
    fn rejects_oversized_payload() {
        let mut e = sample();
        e.payload.text = "x".repeat(PAYLOAD_MAX_BYTES + 1);
        assert_eq!(validate_structure(&e, 1), Err(RejectReason::PayloadTooLarge));
    }

    #[test]
    fn rejects_expired() {
        let mut e = sample();
        e.disclosure.expires_at = 100;
        assert_eq!(validate_structure(&e, 200), Err(RejectReason::Expired));
    }
}
