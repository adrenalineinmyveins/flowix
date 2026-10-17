//! F4 本机 Agent 协作：咨询信箱、预算护栏与咨询元数据（可测核心）。
//!
//! 覆盖 AGENT-LOCAL-CONSULT §3.5/§3.3/§5 的纯逻辑与数据面：事件队列
//! （seq 递增、断线重放、上限 100）、预算判定（used vs limit）、
//! `consults/<id>.json` 元数据与 `events.jsonl` 留痕、发起前校验
//! （profile 存在 + consultable）。subagent spawn 编排依赖 desktop →
//! DSH appserver 的方法调用通道（C-M1 运行时部分），当前未接线——
//! `initiate_consult` 建档后停留在 pending 状态，通道就绪后由编排器消费。

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use flowix_core::agent::AgentProfile;

const MAX_MAILBOX_EVENTS: usize = 100;
pub const DEFAULT_TOKEN_BUDGET: u64 = 50_000;

// ---------------------------------------------------------------------------
// 事件与信箱
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ConsultEvent {
    Result { answer: String, artifact_suggestion: Option<String> },
    Failed { reason: String },
    Askback { question: String, options: Vec<String> },
    BudgetUsed { tokens: u64, remaining: u64 },
    Started { material_count: usize },
}

#[derive(Debug, Default)]
pub struct ConsultMailbox {
    next_seq: u64,
    events: VecDeque<(u64, ConsultEvent)>,
}

impl ConsultMailbox {
    /// 推入事件，返回其 seq。超过上限丢弃最旧（FIFO）。
    pub fn push(&mut self, event: ConsultEvent) -> u64 {
        self.next_seq += 1;
        let seq = self.next_seq;
        self.events.push_back((seq, event));
        while self.events.len() > MAX_MAILBOX_EVENTS {
            self.events.pop_front();
        }
        seq
    }

    /// 取 seq 之后的事件（断线重放：客户端带上最后收到的 seq）。
    pub fn drain_after(&self, after_seq: u64) -> Vec<(u64, ConsultEvent)> {
        self.events
            .iter()
            .filter(|(seq, _)| *seq > after_seq)
            .cloned()
            .collect()
    }

    pub fn latest_seq(&self) -> u64 {
        self.next_seq
    }
}

/// 进程内咨询注册表：consultId → 信箱。
fn registry() -> &'static Mutex<BTreeMap<String, ConsultMailbox>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, ConsultMailbox>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn push_event(consult_id: &str, event: ConsultEvent) -> u64 {
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    registry
        .entry(consult_id.to_string())
        .or_default()
        .push(event)
}

pub fn events_after(consult_id: &str, after_seq: u64) -> Vec<(u64, ConsultEvent)> {
    let registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    registry
        .get(consult_id)
        .map(|mailbox| mailbox.drain_after(after_seq))
        .unwrap_or_default()
}

pub fn latest_seq(consult_id: &str) -> u64 {
    let registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    registry
        .get(consult_id)
        .map(ConsultMailbox::latest_seq)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 预算护栏（纯逻辑）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct BudgetGuard {
    pub limit: u64,
    pub used: u64,
}

impl BudgetGuard {
    pub fn new(limit: u64) -> Self {
        Self { limit, used: 0 }
    }

    pub fn record(&mut self, tokens: u64) {
        self.used = self.used.saturating_add(tokens);
    }

    /// 超限即应取消（used >= limit）。
    pub fn should_cancel(&self) -> bool {
        self.limit > 0 && self.used >= self.limit
    }

    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }
}

// ---------------------------------------------------------------------------
// 咨询元数据（.flowix/agents/<fromId>/consults/）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ConsultStatus {
    Pending,
    Running,
    Completed,
    Failed,
    BudgetExceeded,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConsultMeta {
    pub version: u32,
    pub consult_id: String,
    pub from_agent_id: String,
    pub to_agent_id: String,
    pub status: ConsultStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub child_thread_id: Option<String>,
    pub token_budget: u64,
    pub tokens_used: u64,
}

pub fn consult_meta_path(notebook_root: &Path, from_agent_id: &str, consult_id: &str) -> std::path::PathBuf {
    notebook_root
        .join(".flowix")
        .join("agents")
        .join(from_agent_id)
        .join("consults")
        .join(format!("{consult_id}.json"))
}

pub fn write_consult_meta(notebook_root: &Path, meta: &ConsultMeta) {
    let path = consult_meta_path(notebook_root, &meta.from_agent_id, &meta.consult_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_string_pretty(meta) {
        let _ = std::fs::write(path, encoded);
    }
}

pub fn read_consult_meta(
    notebook_root: &Path,
    from_agent_id: &str,
    consult_id: &str,
) -> Option<ConsultMeta> {
    let path = consult_meta_path(notebook_root, from_agent_id, consult_id);
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 追加咨询事件到 `consults/<id>.events.jsonl`（磁盘留痕，区别于内存信箱）。
pub fn append_consult_log(notebook_root: &Path, meta: &ConsultMeta, event: &ConsultEvent) {
    let path = notebook_root
        .join(".flowix")
        .join("agents")
        .join(&meta.from_agent_id)
        .join("consults")
        .join(format!("{}.events.jsonl", meta.consult_id));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_string(event) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "{encoded}");
        }
    }
}

// ---------------------------------------------------------------------------
// 发起前校验与建档
// ---------------------------------------------------------------------------

pub struct ConsultRequest {
    pub from_agent_id: String,
    pub to_agent_id: String,
    pub token_budget: Option<u64>,
}

/// 发起校验：双方 profile 存在、被咨询方 consultable、预算为正。
/// 返回 (to_profile, budget)。对应设计 §3.1 的 AGENT_NOT_FOUND /
/// consultable=false 拒绝与 BUDGET_INVALID。
pub fn validate_request(
    notebook_root: &Path,
    request: &ConsultRequest,
) -> Result<(AgentProfile, u64), String> {
    let from_path = notebook_root
        .join(".agents")
        .join(&request.from_agent_id)
        .join("profile.md");
    if !from_path.is_file() {
        return Err(format!("AGENT_NOT_FOUND: {}", request.from_agent_id));
    }
    let to_path = notebook_root
        .join(".agents")
        .join(&request.to_agent_id)
        .join("profile.md");
    let to_content = std::fs::read_to_string(&to_path)
        .map_err(|_| format!("AGENT_NOT_FOUND: {}", request.to_agent_id))?;
    let (to_profile, _) = AgentProfile::parse(&to_content)
        .map_err(|_| format!("AGENT_NOT_FOUND: {}", request.to_agent_id))?;
    if !to_profile.consultable {
        return Err(format!("NOT_CONSULTABLE: {}", request.to_agent_id));
    }
    let budget = request.token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET);
    if budget == 0 {
        return Err("BUDGET_INVALID: budget must be positive".to_string());
    }
    Ok((to_profile, budget))
}

/// 建档：生成 consultId、写元数据、建内存信箱并推 Started。
/// subagent spawn 编排未接线前，状态停留在 Pending。
pub fn initiate_consult(
    notebook_root: &Path,
    request: ConsultRequest,
) -> Result<ConsultMeta, String> {
    let (_to_profile, budget) = validate_request(notebook_root, &request)?;
    let consult_id = format!("cst_{}", uuid::Uuid::new_v4().simple());
    let now = chrono::Utc::now().timestamp_millis();
    let meta = ConsultMeta {
        version: 1,
        consult_id: consult_id.clone(),
        from_agent_id: request.from_agent_id,
        to_agent_id: request.to_agent_id,
        status: ConsultStatus::Pending,
        created_at: now,
        updated_at: now,
        child_thread_id: None,
        token_budget: budget,
        tokens_used: 0,
    };
    write_consult_meta(notebook_root, &meta);
    push_event(&consult_id, ConsultEvent::Started { material_count: 0 });
    tracing::info!(
        consult_id = %consult_id,
        "consult registered (orchestrator spawn pending DSH method channel)"
    );
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_assigns_incrementing_seqs_and_replays_after() {
        let mut mailbox = ConsultMailbox::default();
        mailbox.push(ConsultEvent::Started { material_count: 2 });
        let seq2 = mailbox.push(ConsultEvent::Result {
            answer: "ok".into(),
            artifact_suggestion: None,
        });
        assert_eq!(seq2, 2);
        // 断线重放：从 seq 1 之后取，拿到 seq 2。
        let replay = mailbox.drain_after(1);
        assert_eq!(replay.len(), 1);
        assert!(matches!(replay[0].1, ConsultEvent::Result { .. }));
        // 从头取拿到全部。
        assert_eq!(mailbox.drain_after(0).len(), 2);
        assert_eq!(mailbox.latest_seq(), 2);
    }

    #[test]
    fn mailbox_caps_at_100_events() {
        let mut mailbox = ConsultMailbox::default();
        for i in 0..120 {
            mailbox.push(ConsultEvent::Failed { reason: format!("r{i}") });
        }
        assert_eq!(mailbox.events.len(), MAX_MAILBOX_EVENTS);
        // 最旧的是 r20（0..=119，丢前 20）。
        assert!(matches!(
            &mailbox.events.front().unwrap().1,
            ConsultEvent::Failed { reason } if reason == "r20"
        ));
    }

    #[test]
    fn budget_guard_cancels_at_limit() {
        let mut guard = BudgetGuard::new(1000);
        assert!(!guard.should_cancel());
        guard.record(600);
        assert!(!guard.should_cancel());
        assert_eq!(guard.remaining(), 400);
        guard.record(400);
        assert!(guard.should_cancel());
        assert_eq!(guard.remaining(), 0);
        // 溢出保护。
        guard.record(u64::MAX);
        assert_eq!(guard.used, u64::MAX);
    }

    #[test]
    fn consult_meta_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let meta = ConsultMeta {
            version: 1,
            consult_id: "cst_x".into(),
            from_agent_id: "agt_a".into(),
            to_agent_id: "agt_b".into(),
            status: ConsultStatus::Running,
            created_at: 1,
            updated_at: 2,
            child_thread_id: Some("thread_1".into()),
            token_budget: 5000,
            tokens_used: 100,
        };
        write_consult_meta(temp.path(), &meta);
        let read = read_consult_meta(temp.path(), "agt_a", "cst_x").unwrap();
        assert_eq!(read, meta);
        assert!(read_consult_meta(temp.path(), "agt_a", "cst_missing").is_none());
    }

    #[test]
    fn validate_and_initiate_reject_bad_requests() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // to 不存在。
        let err = validate_request(
            root,
            &ConsultRequest {
                from_agent_id: "agt_a".into(),
                to_agent_id: "agt_missing".into(),
                token_budget: None,
            },
        )
        .unwrap_err();
        assert!(err.starts_with("AGENT_NOT_FOUND"), "{err}");

        // 建双方 profile（from 最小文件即可，to 需合法 profile）。
        write_min_profile(root, "agt_a");
        write_profile(root, "agt_b", true);
        // to consultable=false。
        write_profile(root, "agt_c", false);
        let err = validate_request(
            root,
            &ConsultRequest {
                from_agent_id: "agt_a".into(),
                to_agent_id: "agt_c".into(),
                token_budget: None,
            },
        )
        .unwrap_err();
        assert!(err.starts_with("NOT_CONSULTABLE"), "{err}");
        // 预算 0。
        let err = validate_request(
            root,
            &ConsultRequest {
                from_agent_id: "agt_a".into(),
                to_agent_id: "agt_b".into(),
                token_budget: Some(0),
            },
        )
        .unwrap_err();
        assert!(err.starts_with("BUDGET_INVALID"), "{err}");
        // 合法发起：建档 + 信箱有 Started。
        let meta = initiate_consult(
            root,
            ConsultRequest {
                from_agent_id: "agt_a".into(),
                to_agent_id: "agt_b".into(),
                token_budget: Some(1234),
            },
        )
        .unwrap();
        assert_eq!(meta.status, ConsultStatus::Pending);
        assert!(read_consult_meta(root, "agt_a", &meta.consult_id).is_some());
        assert_eq!(latest_seq(&meta.consult_id), 1);
        assert!(matches!(
            events_after(&meta.consult_id, 0)[0].1,
            ConsultEvent::Started { .. }
        ));
    }

    fn write_min_profile(root: &Path, agent_id: &str) {
        let dir = root.join(".agents").join(agent_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.md"), "占位").unwrap();
    }

    fn write_profile(root: &Path, agent_id: &str, consultable: bool) {
        let profile = AgentProfile {
            agent_id: agent_id.into(),
            display_name: format!("Agent {agent_id}"),
            bio: String::new(),
            expertise: vec![],
            agent_type: "dsh".into(),
            model: None,
            consultable,
            redaction_level: None,
            disclosure_scope: vec![],
            created_at: 1,
            updated_at: 1,
        };
        let dir = root.join(".agents").join(agent_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.md"), profile.serialize("")).unwrap();
    }
}
