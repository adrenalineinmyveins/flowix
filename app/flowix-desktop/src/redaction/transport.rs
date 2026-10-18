//! F5 客户端传输层（阶段三 · 客户端协议面切片）。
//!
//! 唯一出口纪律不变：出设备字节只能来自 `take_confirmed_payload`
//! （与 `redaction_send` 同源，字节与留档 byte-equal）。本模块把
//! confirmed 信封投递到配置的平台中继；投递失败不影响留档状态
//! （take 幂等，可重试），云端协议契约（disclosure 元数据执法等）属
//! FD-10 立项后的中继侧工作。

use std::path::Path;

use serde::Serialize;

use super::outbox::take_confirmed_payload;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayDeliverResult {
    pub receipt_id: String,
    pub byte_length: usize,
    pub http_status: u16,
}

/// 校验中继地址：必须是 http(s) 绝对 URL。
pub fn validate_relay_url(relay_url: &str) -> Result<(), String> {
    let trimmed = relay_url.trim();
    if trimmed.is_empty() {
        return Err("RELAY_NOT_CONFIGURED".to_string());
    }
    let parsed = trimmed
        .parse::<reqwest::Url>()
        .map_err(|_| "RELAY_URL_INVALID".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("RELAY_URL_INVALID".to_string());
    }
    Ok(())
}

/// 从唯一出口取 confirmed 信封字节并投递到中继。
///
/// - 未过确认门：`UNCONFIRMED_SEND_BLOCKED:*`（与 redaction_send 同一兜底）
/// - 投递失败（网络错误 / 非 2xx）：返回 Err，留档保持 confirmed 可重试
pub async fn deliver_confirmed_envelope(
    notebook_root: &Path,
    receipt_id: &str,
    relay_url: &str,
) -> Result<RelayDeliverResult, String> {
    validate_relay_url(relay_url)?;
    // 唯一出口：字节与留档逐字节一致（幂等取出）。
    let bytes = take_confirmed_payload(notebook_root, receipt_id)?;
    let byte_length = bytes.len();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("RELAY_CLIENT_INIT_FAILED:{error}"))?;
    let response = client
        .post(relay_url.trim())
        .header("content-type", "application/json")
        .header("x-flowix-envelope", "flowix.consult.v1")
        .body(bytes)
        .send()
        .await
        .map_err(|error| format!("RELAY_DELIVERY_FAILED:{error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("RELAY_REJECTED:{}", status.as_u16()));
    }
    Ok(RelayDeliverResult {
        receipt_id: receipt_id.to_string(),
        byte_length,
        http_status: status.as_u16(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::redaction::outbox::{draft_from_preview, settle, write_record, OutboxStatus};
    use crate::redaction::RedactionPreviewReport;

    fn report() -> RedactionPreviewReport {
        RedactionPreviewReport {
            receipt_id: String::new(),
            final_text: "某电商公司 Q3 复购下滑".into(),
            placeholder_map: Default::default(),
            l1_hits: vec![],
            attempts: 1,
            used_fallback: false,
            included_paths: vec!["a.md".into()],
            dropped_paths: vec![],
            truncated: false,
            target_level: "r2".into(),
            rule_set_version: "test".into(),
        }
    }

    #[test]
    fn relay_url_validation() {
        assert_eq!(validate_relay_url("").unwrap_err(), "RELAY_NOT_CONFIGURED");
        assert_eq!(
            validate_relay_url("   ").unwrap_err(),
            "RELAY_NOT_CONFIGURED"
        );
        assert_eq!(validate_relay_url("ftp://x").unwrap_err(), "RELAY_URL_INVALID");
        assert_eq!(
            validate_relay_url("not a url").unwrap_err(),
            "RELAY_URL_INVALID"
        );
        assert!(validate_relay_url("https://relay.example/v1/consult").is_ok());
        assert!(validate_relay_url("http://127.0.0.1:8080/v1").is_ok());
    }

    #[tokio::test]
    async fn deliver_blocks_unconfirmed_and_keeps_confirmed_retryable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // draft 态：唯一出口兜底拦截。
        let draft = draft_from_preview("q", &["nb".into()], &report(), 1);
        let receipt = draft.receipt_id.clone();
        write_record(root, &draft).unwrap();
        let error = deliver_confirmed_envelope(root, &receipt, "http://127.0.0.1:9/v1")
            .await
            .unwrap_err();
        assert!(error.contains("UNCONFIRMED_SEND_BLOCKED"), "{error}");

        // confirmed 态 + 不可达中继：投递失败，但留档保持 confirmed（幂等可重试）。
        settle(root, &receipt, OutboxStatus::Confirmed).unwrap();
        let error = deliver_confirmed_envelope(root, &receipt, "http://127.0.0.1:9/v1")
            .await
            .unwrap_err();
        assert!(
            error.starts_with("RELAY_DELIVERY_FAILED"),
            "{error}"
        );
        let record = crate::redaction::read_record(root, &receipt)
            .expect("record still present after failed delivery");
        assert_eq!(record.status, OutboxStatus::Confirmed);
        // 重试仍能从出口取到字节（byte-equal 纪律）。
        assert!(!take_confirmed_payload(root, &receipt).unwrap().is_empty());

        // 未配置中继。
        let error = deliver_confirmed_envelope(root, &receipt, "").await.unwrap_err();
        assert_eq!(error, "RELAY_NOT_CONFIGURED");
    }
}
