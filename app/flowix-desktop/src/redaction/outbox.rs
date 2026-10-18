//! F6 M3 出站留档（REDACTION-PIPELINE.md §6）。
//!
//! `<notebook>/.flowix/redaction-outbox/rct_<id>.json`：完整信封 + 占位符映射 +
//! 状态机 `draft → confirmed / cancelled / failed`。上限 200 条，FIFO 清理。
//! 留档在 `.flowix/` 下——按既有 dot-dir 策略不进 FTS 索引、不参与云同步，
//! 属设备级审计数据。
//!
//! 发送纪律：出设备字节必须来自 `take_confirmed_payload`——只有 `confirmed`
//! 状态的留档能取出信封字节（`UNCONFIRMED_SEND_BLOCKED` 兜底），且取出的
//! 字节与留档重读后的信封字节逐字节一致（byte-equal 验收）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flowix_redaction::RuleHit;
use serde::{Deserialize, Serialize};

pub const OUTBOX_DIR_NAME: &str = "redaction-outbox";
/// 留档上限（FIFO 清理阈值）。
pub const OUTBOX_LIMIT: usize = 200;
/// F5 披露标记默认值（REDACTION-PIPELINE §5.1）：对端转咨深度与载荷寿命。
pub const DISCLOSURE_MAX_HOPS: u32 = 2;
/// 载荷默认 7 天过期（秒；wire v1 契约统一 Unix 秒）。
pub const DISCLOSURE_TTL_SECS: i64 = 7 * 24 * 3600;

/// wire v1 路由方（from/to）。serde 与 flowix_relay::envelope::AgentRef 同形。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentParty {
    pub agent_id: String,
    #[serde(default)]
    pub display_name: String,
}

fn default_from_agent() -> AgentParty {
    AgentParty { agent_id: "local".to_string(), display_name: String::new() }
}

fn default_to_agent() -> AgentParty {
    // 无显式收件人 = 手动投递语义；网络投递时中继按 RECIPIENT_UNKNOWN 拒。
    AgentParty { agent_id: "manual".to_string(), display_name: String::new() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutboxStatus {
    Draft,
    Confirmed,
    Cancelled,
    Failed,
}

/// 出设备信封（wire v1 `flowix.consult.v1`）：发送字节 = 该结构的确定性
/// JSON 序列化。类型复用 flowix-relay（协议单一来源）；结构校验在
/// 中继入口执行，本端构造保证字段完备。
pub type RedactionEnvelope = flowix_relay::envelope::WireEnvelope;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionOutboxRecord {
    pub receipt_id: String,
    pub status: OutboxStatus,
    pub created_at: i64,
    pub settled_at: Option<i64>,
    pub question: String,
    pub scope_entry_ids: Vec<String>,
    /// 当次授权范围标签（tag:/path: 前缀），进信封 disclosure.allowedScopes。
    #[serde(default)]
    pub allowed_scopes: Vec<String>,
    pub target_level: String,
    pub rule_set_version: String,
    pub payload: String,
    pub placeholder_map: BTreeMap<String, String>,
    pub l1_hits: Vec<RuleHit>,
    pub included_paths: Vec<String>,
    pub dropped_paths: Vec<String>,
    pub attempts: u32,
    pub used_fallback: bool,
    pub truncated: bool,
    /// F5 wire v1 路由：发起方（本机 agent），缺省 local。
    #[serde(default = "default_from_agent")]
    pub from_agent: AgentParty,
    /// F5 wire v1 路由：收件方，缺省 manual（手动投递语义）。
    #[serde(default = "default_to_agent")]
    pub to_agent: AgentParty,
}

impl RedactionOutboxRecord {
    /// 设置 wire v1 路由双方（preview 编排层按咨询目标填入）。
    pub fn with_parties(mut self, from: AgentParty, to: AgentParty) -> Self {
        self.from_agent = from;
        self.to_agent = to;
        self
    }

    pub fn envelope(&self) -> RedactionEnvelope {
        use flowix_relay::envelope::{
            AgentRef, WireDisclosure, WirePayload, WireProvenance, ENVELOPE_VERSION,
        };
        // created_at 为毫秒；wire v1 契约统一 Unix 秒。
        let created_secs = self.created_at / 1000;
        RedactionEnvelope {
            envelope: ENVELOPE_VERSION.to_string(),
            from: AgentRef {
                agent_id: self.from_agent.agent_id.clone(),
                display_name: self.from_agent.display_name.clone(),
            },
            to: AgentRef {
                agent_id: self.to_agent.agent_id.clone(),
                display_name: self.to_agent.display_name.clone(),
            },
            in_reply_to: None,
            disclosure: WireDisclosure {
                redaction_level: self.target_level.clone(),
                max_hops: DISCLOSURE_MAX_HOPS,
                hops_remaining: DISCLOSURE_MAX_HOPS,
                expires_at: created_secs + DISCLOSURE_TTL_SECS,
                allowed_scopes: self.allowed_scopes.clone(),
            },
            payload: WirePayload {
                kind: "consultation".to_string(),
                text: self.payload.clone(),
            },
            provenance: WireProvenance {
                pipeline_version: "1".to_string(),
                rule_set_version: self.rule_set_version.clone(),
                used_fallback: self.used_fallback,
                confirmed_by: "user".to_string(),
                confirmed_at: created_secs,
                receipt_id: self.receipt_id.clone(),
            },
        }
    }
}

pub fn new_receipt_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    format!("rct_{}", &raw[..12])
}

pub fn outbox_dir(notebook_root: &Path) -> PathBuf {
    notebook_root.join(".flowix").join(OUTBOX_DIR_NAME)
}

fn record_path(dir: &Path, receipt_id: &str) -> PathBuf {
    dir.join(format!("{receipt_id}.json"))
}

impl RedactionOutboxRecord {
    /// 确定性序列化：serde_json 对固定字段的 struct 输出稳定，
    /// 同一留档重读后再次序列化逐字节一致（byte-equal 验收依据）。
    pub fn envelope_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.envelope()).expect("envelope serialize")
    }
}

/// 从 preview 报告构造 draft 留档（纯函数，不落盘）。
pub fn draft_from_preview(
    question: &str,
    scope_entry_ids: &[String],
    report: &crate::redaction::RedactionPreviewReport,
    now_ms: i64,
) -> RedactionOutboxRecord {
    RedactionOutboxRecord {
        receipt_id: new_receipt_id(),
        status: OutboxStatus::Draft,
        created_at: now_ms,
        settled_at: None,
        question: question.to_string(),
        scope_entry_ids: scope_entry_ids.to_vec(),
        allowed_scopes: Vec::new(),
        target_level: report.target_level.clone(),
        rule_set_version: report.rule_set_version.clone(),
        payload: report.final_text.clone(),
        placeholder_map: report.placeholder_map.clone(),
        l1_hits: report.l1_hits.clone(),
        included_paths: report.included_paths.clone(),
        dropped_paths: report.dropped_paths.clone(),
        attempts: report.attempts,
        used_fallback: report.used_fallback,
        truncated: report.truncated,
        from_agent: default_from_agent(),
        to_agent: default_to_agent(),
    }
}

/// 写留档（tmp + rename 原子写）并执行 FIFO 清理。
pub fn write_record(
    notebook_root: &Path,
    record: &RedactionOutboxRecord,
) -> Result<(), String> {
    let dir = outbox_dir(notebook_root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("outbox_dir_create_failed: {e}"))?;
    let raw = serde_json::to_string_pretty(record)
        .map_err(|e| format!("outbox_serialize_failed: {e}"))?;
    let target = record_path(&dir, &record.receipt_id);
    let tmp = dir.join(format!("{}.tmp", record.receipt_id));
    std::fs::write(&tmp, raw.as_bytes()).map_err(|e| format!("outbox_write_failed: {e}"))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("outbox_write_failed: {e}"))?;
    enforce_fifo(&dir);
    Ok(())
}

fn parse_record(path: &Path) -> Option<RedactionOutboxRecord> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 全量留档（created_at 升序，旧 → 新）。坏文件跳过。
pub fn list_records(notebook_root: &Path) -> Vec<RedactionOutboxRecord> {
    let dir = outbox_dir(notebook_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut records: Vec<RedactionOutboxRecord> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.starts_with("rct_"))
        })
        .filter_map(|path| parse_record(&path))
        .collect();
    records.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.receipt_id.cmp(&b.receipt_id))
    });
    records
}

pub fn read_record(
    notebook_root: &Path,
    receipt_id: &str,
) -> Option<RedactionOutboxRecord> {
    parse_record(&record_path(&outbox_dir(notebook_root), receipt_id))
}

/// 状态迁移：draft → confirmed / cancelled / failed。
/// 非 draft 状态重复结算 → `ALREADY_SETTLED`。
pub fn settle(
    notebook_root: &Path,
    receipt_id: &str,
    status: OutboxStatus,
) -> Result<RedactionOutboxRecord, String> {
    let mut record = read_record(notebook_root, receipt_id)
        .ok_or_else(|| "RECEIPT_NOT_FOUND".to_string())?;
    if record.status != OutboxStatus::Draft {
        return Err(format!("ALREADY_SETTLED:{}", status_name(record.status)));
    }
    record.status = status;
    record.settled_at = Some(chrono::Utc::now().timestamp_millis());
    write_record(notebook_root, &record)?;
    Ok(record)
}

pub fn status_name(status: OutboxStatus) -> &'static str {
    match status {
        OutboxStatus::Draft => "draft",
        OutboxStatus::Confirmed => "confirmed",
        OutboxStatus::Cancelled => "cancelled",
        OutboxStatus::Failed => "failed",
    }
}

/// 唯一发送出口：只有 confirmed 留档能取出信封字节。
/// draft / cancelled / failed / 不存在一律 `UNCONFIRMED_SEND_BLOCKED`。
pub fn take_confirmed_payload(
    notebook_root: &Path,
    receipt_id: &str,
) -> Result<Vec<u8>, String> {
    let record = read_record(notebook_root, receipt_id)
        .ok_or_else(|| "UNCONFIRMED_SEND_BLOCKED:receipt_not_found".to_string())?;
    if record.status != OutboxStatus::Confirmed {
        return Err(format!(
            "UNCONFIRMED_SEND_BLOCKED:status_{}",
            status_name(record.status)
        ));
    }
    Ok(record.envelope_bytes())
}

fn enforce_fifo(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(i64, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.starts_with("rct_"))
        })
        .filter_map(|path| {
            let record = parse_record(&path)?;
            Some((record.created_at, path))
        })
        .collect();
    if files.len() <= OUTBOX_LIMIT {
        return;
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let excess = files.len() - OUTBOX_LIMIT;
    for (_, path) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::RedactionPreviewReport;

    fn report(payload: &str) -> RedactionPreviewReport {
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        RedactionPreviewReport {
            receipt_id: String::new(),
            final_text: format!("某客户 {payload} ⟦PHONE-1⟧ 咨询物流"),
            placeholder_map: map,
            l1_hits: vec![RuleHit {
                rule_id: "phone".to_string(),
                count: 1,
            }],
            attempts: 1,
            used_fallback: false,
            included_paths: vec!["工作/a.md".to_string()],
            dropped_paths: vec![],
            truncated: false,
            target_level: "r2".to_string(),
            rule_set_version: "l1-v1".to_string(),
        }
    }

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn draft_write_read_round_trip() {
        let root = root();
        let record = draft_from_preview(
            "问一下物流",
            &["entry-1".to_string()],
            &report("张工"),
            1000,
        );
        assert_eq!(record.status, OutboxStatus::Draft);
        assert!(record.receipt_id.starts_with("rct_"));
        write_record(root.path(), &record).unwrap();
        let loaded = read_record(root.path(), &record.receipt_id).unwrap();
        assert_eq!(loaded, record);
    }

    /// F5 披露标记：wire v1 信封携带 disclosure 段（等级/hops/过期/范围）
    /// 与 from/to 路由，且确定性序列化保持 byte-equal 纪律。
    #[test]
    fn envelope_carries_disclosure_block() {
        let mut record = draft_from_preview("q", &["e1".to_string()], &report("x"), 1000);
        record.allowed_scopes = vec!["tag:finance/财报".to_string()];
        record = record.with_parties(
            AgentParty { agent_id: "agt_a".to_string(), display_name: "甲".to_string() },
            AgentParty { agent_id: "agt_b".to_string(), display_name: "乙".to_string() },
        );
        let envelope = record.envelope();
        assert_eq!(envelope.envelope, "flowix.consult.v1");
        assert_eq!(envelope.from.agent_id, "agt_a");
        assert_eq!(envelope.to.agent_id, "agt_b");
        assert_eq!(envelope.payload.kind, "consultation");
        assert_eq!(envelope.payload.text, record.payload);
        assert_eq!(envelope.provenance.receipt_id, record.receipt_id);
        assert_eq!(envelope.disclosure.redaction_level, "r2");
        assert_eq!(envelope.disclosure.max_hops, DISCLOSURE_MAX_HOPS);
        assert_eq!(envelope.disclosure.hops_remaining, DISCLOSURE_MAX_HOPS);
        // created_at 毫秒 → wire 秒制：1000ms = 1s。
        assert_eq!(envelope.disclosure.expires_at, 1 + DISCLOSURE_TTL_SECS);
        assert_eq!(envelope.disclosure.allowed_scopes, vec!["tag:finance/财报"]);
        let raw = String::from_utf8(record.envelope_bytes()).unwrap();
        assert!(raw.contains("\"disclosure\""));
        assert!(raw.contains("\"allowedScopes\""));
        // wire v1 全键在场（中继结构校验的输入形态）。
        assert!(raw.contains("\"flowix.consult.v1\""));
        assert!(raw.contains("\"provenance\""));
        // 旧档（无 allowedScopes/路由字段）仍可读，scope 为空、路由取默认。
        let legacy = draft_from_preview("q", &[], &report("y"), 1);
        assert!(legacy.allowed_scopes.is_empty());
        assert_eq!(legacy.from_agent.agent_id, "local");
        assert_eq!(legacy.to_agent.agent_id, "manual");
        assert!(legacy.envelope().disclosure.allowed_scopes.is_empty());
    }

    #[test]
    fn settle_state_machine() {
        let root = root();
        let record = draft_from_preview("q", &[], &report("x"), 1);
        write_record(root.path(), &record).unwrap();

        let confirmed = settle(root.path(), &record.receipt_id, OutboxStatus::Confirmed)
            .unwrap();
        assert_eq!(confirmed.status, OutboxStatus::Confirmed);
        assert!(confirmed.settled_at.is_some());

        let again = settle(root.path(), &record.receipt_id, OutboxStatus::Cancelled);
        assert!(again.unwrap_err().starts_with("ALREADY_SETTLED"));

        let other = draft_from_preview("q", &[], &report("y"), 2);
        write_record(root.path(), &other).unwrap();
        let cancelled = settle(root.path(), &other.receipt_id, OutboxStatus::Cancelled)
            .unwrap();
        assert_eq!(cancelled.status, OutboxStatus::Cancelled);

        assert_eq!(
            settle(root.path(), "rct_missing", OutboxStatus::Confirmed)
                .unwrap_err(),
            "RECEIPT_NOT_FOUND"
        );
    }

    #[test]
    fn send_gate_and_byte_equal() {
        let root = root();
        let draft = draft_from_preview("q", &[], &report("z"), 3);
        write_record(root.path(), &draft).unwrap();

        let blocked = take_confirmed_payload(root.path(), &draft.receipt_id).unwrap_err();
        assert_eq!(blocked, "UNCONFIRMED_SEND_BLOCKED:status_draft");

        settle(root.path(), &draft.receipt_id, OutboxStatus::Confirmed).unwrap();
        let sent = take_confirmed_payload(root.path(), &draft.receipt_id).unwrap();

        let reread = read_record(root.path(), &draft.receipt_id).unwrap();
        assert_eq!(sent, reread.envelope_bytes());
        assert_eq!(sent, take_confirmed_payload(root.path(), &draft.receipt_id).unwrap());

        let envelope: RedactionEnvelope = serde_json::from_slice(&sent).unwrap();
        assert_eq!(envelope.provenance.receipt_id, draft.receipt_id);
        assert_eq!(envelope.payload.text, draft.payload);
        assert_eq!(envelope.payload.kind, "consultation");
        assert_eq!(envelope.disclosure.redaction_level, "r2");
    }

    #[test]
    fn cancelled_and_failed_block_send() {
        let root = root();
        let cancelled = draft_from_preview("q", &[], &report("a"), 4);
        write_record(root.path(), &cancelled).unwrap();
        settle(root.path(), &cancelled.receipt_id, OutboxStatus::Cancelled).unwrap();
        assert_eq!(
            take_confirmed_payload(root.path(), &cancelled.receipt_id).unwrap_err(),
            "UNCONFIRMED_SEND_BLOCKED:status_cancelled"
        );

        let failed = draft_from_preview("q", &[], &report("b"), 5);
        write_record(root.path(), &failed).unwrap();
        settle(root.path(), &failed.receipt_id, OutboxStatus::Failed).unwrap();
        assert_eq!(
            take_confirmed_payload(root.path(), &failed.receipt_id).unwrap_err(),
            "UNCONFIRMED_SEND_BLOCKED:status_failed"
        );

        assert!(take_confirmed_payload(root.path(), "rct_nope").is_err());
    }

    #[test]
    fn fifo_keeps_newest_200() {
        let root = root();
        for i in 0..(OUTBOX_LIMIT + 5) {
            let record = draft_from_preview("q", &[], &report("x"), i as i64);
            write_record(root.path(), &record).unwrap();
        }
        let remaining = list_records(root.path());
        assert_eq!(remaining.len(), OUTBOX_LIMIT);
        assert_eq!(remaining.first().unwrap().created_at, 5);
        assert_eq!(remaining.last().unwrap().created_at, OUTBOX_LIMIT as i64 + 4);
    }

    #[test]
    fn receipt_id_unique_and_shaped() {
        let a = new_receipt_id();
        let b = new_receipt_id();
        assert_ne!(a, b);
        assert!(a.starts_with("rct_"));
        assert_eq!(a.len(), "rct_".len() + 12);
    }
}
