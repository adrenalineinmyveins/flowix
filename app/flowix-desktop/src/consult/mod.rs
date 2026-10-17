//! F4 本机 Agent 协作：咨询信箱、预算护栏、咨询元数据与编排。
//!
//! 覆盖 AGENT-LOCAL-CONSUST §3.1–§3.5：事件队列（seq 递增、断线重放、
//! 上限 100）、预算判定、`consults/<id>.json` 元数据与 `events.jsonl`
//! 留痕、发起前校验（profile 存在 + consultable）。编排经 deepseek_harness
//! 的 stdio JSON-RPC 主机调用 `thread/subagent/*`（B 是 A 的子线程，天然
//! 受 maxDepth=4 / maxChildren=16 约束）。

pub mod orchestrator;

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

/// 组装给 B 的咨询提示词（简版 context builder，设计 §3.2）。
///
/// 素材注入（发起方 disclosure_scope 内笔记围栏化）属 C-M1 完整版；
/// 本版注入 B 的专长画像与问题，供子线程冷启动。
pub fn build_consult_prompt(
    to_profile: &AgentProfile,
    from_display_name: &str,
    question: &str,
) -> String {
    let expertise = if to_profile.expertise.is_empty() {
        "（未声明）".to_string()
    } else {
        to_profile.expertise.join(", ")
    };
    format!(
        "你是 Agent「{}」，专长：{}。\n\
另一位 Agent「{}」向你咨询。\n\n\
<question>\n{}\n</question>\n\n\
请基于你的专长作答。回答将作为咨询结果返回给咨询方。",
        to_profile.display_name, expertise, from_display_name, question.trim()
    )
}

/// 建档（无运行时依赖，供测试）：校验 → 元数据 → 信箱 → prompt。
pub fn register_consult(
    notebook_root: &Path,
    request: &ConsultRequest,
    question: &str,
) -> Result<(ConsultMeta, String), String> {
    let (to_profile, budget) = validate_request(notebook_root, request)?;
    let consult_id = format!("cst_{}", uuid::Uuid::new_v4().simple());
    let now = chrono::Utc::now().timestamp_millis();
    let meta = ConsultMeta {
        version: 1,
        consult_id: consult_id.clone(),
        from_agent_id: request.from_agent_id.clone(),
        to_agent_id: request.to_agent_id.clone(),
        status: ConsultStatus::Pending,
        created_at: now,
        updated_at: now,
        child_thread_id: None,
        token_budget: budget,
        tokens_used: 0,
    };
    write_consult_meta(notebook_root, &meta);
    push_event(&consult_id, ConsultEvent::Started { material_count: 0 });
    let from_display_name = read_display_name(notebook_root, &request.from_agent_id)
        .unwrap_or_else(|| request.from_agent_id.clone());
    let prompt = build_consult_prompt(&to_profile, &from_display_name, question);
    Ok((meta, prompt))
}

/// 建档并 spawn 编排 task：register → 异步跑完整周期。
pub fn initiate_consult(
    app: &tauri::AppHandle,
    notebook_root: &Path,
    request: ConsultRequest,
    question: String,
) -> Result<ConsultMeta, String> {
    let (meta, prompt) = register_consult(notebook_root, &request, &question)?;
    let app = app.clone();
    let root = notebook_root.to_path_buf();
    let meta_for_task = meta.clone();
    tauri::async_runtime::spawn(async move {
        orchestrator::orchestrate(&app, &root, meta_for_task, prompt).await;
    });
    Ok(meta)
}

fn read_display_name(notebook_root: &Path, agent_id: &str) -> Option<String> {
    let content = std::fs::read_to_string(
        notebook_root.join(".agents").join(agent_id).join("profile.md"),
    )
    .ok()?;
    AgentProfile::parse(&content).ok().map(|(profile, _)| profile.display_name)
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
        // 合法发起：建档 + 信箱有 Started + prompt 含问题与专长。
        let (meta, prompt) = register_consult(
            root,
            &ConsultRequest {
                from_agent_id: "agt_a".into(),
                to_agent_id: "agt_b".into(),
                token_budget: Some(1234),
            },
            "复购率下滑怎么归因？",
        )
        .unwrap();
        assert_eq!(meta.status, ConsultStatus::Pending);
        assert_eq!(meta.token_budget, 1234);
        assert!(read_consult_meta(root, "agt_a", &meta.consult_id).is_some());
        assert_eq!(latest_seq(&meta.consult_id), 1);
        assert!(matches!(
            events_after(&meta.consult_id, 0)[0].1,
            ConsultEvent::Started { .. }
        ));
        assert!(prompt.contains("复购率下滑怎么归因？"));
        assert!(prompt.contains("<question>"));
    }

    #[test]
    fn consult_prompt_includes_expertise_and_names() {
        let to = AgentProfile {
            agent_id: "agt_b".into(),
            display_name: "财报专家".into(),
            bio: String::new(),
            expertise: vec!["finance/财报".into()],
            agent_type: "dsh".into(),
            model: None,
            consultable: true,
            redaction_level: None,
            disclosure_scope: vec![],
            created_at: 1,
            updated_at: 1,
        };
        let prompt = build_consult_prompt(&to, "电商运营", "Q3 复购为何下滑");
        assert!(prompt.contains("财报专家"));
        assert!(prompt.contains("finance/财报"));
        assert!(prompt.contains("电商运营"));
        // 空专长兜底。
        let mut bare = to.clone();
        bare.expertise = vec![];
        assert!(build_consult_prompt(&bare, "x", "y").contains("（未声明）"));
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
