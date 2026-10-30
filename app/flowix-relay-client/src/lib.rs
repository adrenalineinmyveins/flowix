//! Relay 网络客户端共享栈：自 desktop redaction 抽取，桌面/移动共用。

pub mod account;
pub mod e2e;
pub mod inbound;
pub mod outbox;
pub mod transport;

use std::collections::BTreeMap;

use flowix_redaction::RuleHit;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionPreviewReport {
    /// 关联 outbox 留档（draft 态），确认门与发送出口都以它为凭据。
    pub receipt_id: String,
    pub final_text: String,
    pub placeholder_map: BTreeMap<String, String>,
    pub l1_hits: Vec<RuleHit>,
    pub attempts: u32,
    pub used_fallback: bool,
    pub included_paths: Vec<String>,
    pub dropped_paths: Vec<String>,
    pub truncated: bool,
    pub target_level: String,
    pub rule_set_version: String,
    /// FD-12 M1：出站形态（`zero-material` | `open-scope` | `fallback` | `full`）。
    pub distill_mode: String,
}
