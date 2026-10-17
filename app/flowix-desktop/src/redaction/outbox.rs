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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutboxStatus {
    Draft,
    Confirmed,
    Cancelled,
    Failed,
}

/// 出设备信封：发送字节 = 该结构的确定性 JSON 序列化。
/// 只含对端需要的内容 + 可解释性元数据（ruleSetVersion 供旧留档解释）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionEnvelope {
    pub receipt_id: String,
    pub payload: String,
    pub target_level: String,
    pub rule_set_version: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionOutboxRecord {
    pub receipt_id: String,
    pub status: OutboxStatus,
    pub created_at: i64,
    pub settled_at: Option<i64>,
    pub question: String,
    pub scope_entry_ids: Vec<String>,
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
    pub fn envelope(&self) -> RedactionEnvelope {
        RedactionEnvelope {
            receipt_id: self.receipt_id.clone(),
            payload: self.payload.clone(),
            target_level: self.target_level.clone(),
            rule_set_version: self.rule_set_version.clone(),
            created_at: self.created_at,
        }
    }

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
        assert_eq!(envelope.receipt_id, draft.receipt_id);
        assert_eq!(envelope.payload, draft.payload);
        assert_eq!(envelope.target_level, "r2");
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
