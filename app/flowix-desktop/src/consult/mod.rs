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
    Result {
        answer: String,
        artifact_suggestion: Option<String>,
        /// AGENT-SKILLS P2：本次咨询实际注入生效的技能 id（元数据回流；
        /// 旧档缺省空，兼容 events.jsonl 回放）。
        #[serde(default)]
        applied_skills: Vec<String>,
    },
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
// AskBackGate：反问答复通道（C-M3）
// ---------------------------------------------------------------------------

/// 等待答复的 askback 会话：consultId → 答复发送端。编排器注册后阻塞
/// 等待；`consult_askback_reply` IPC 把答复送入通道，由编排器经
/// `thread/subagent/send` 注入 B 的线程。
fn askback_waiters(
) -> &'static Mutex<BTreeMap<String, tokio::sync::mpsc::Sender<String>>> {
    static WAITERS: OnceLock<Mutex<BTreeMap<String, tokio::sync::mpsc::Sender<String>>>> =
        OnceLock::new();
    WAITERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn register_askback_waiter(consult_id: &str, tx: tokio::sync::mpsc::Sender<String>) {
    let mut waiters = askback_waiters().lock().unwrap_or_else(|e| e.into_inner());
    waiters.insert(consult_id.to_string(), tx);
}

/// 提交反问答复。返回 false 表示当前没有在等待答复的咨询（已超时或已结束）。
pub fn reply_askback(consult_id: &str, answer: String) -> bool {
    let mut waiters = askback_waiters().lock().unwrap_or_else(|e| e.into_inner());
    match waiters.remove(consult_id) {
        Some(tx) => tx.try_send(answer).is_ok(),
        None => false,
    }
}

pub fn has_askback_waiter(consult_id: &str) -> bool {
    let waiters = askback_waiters().lock().unwrap_or_else(|e| e.into_inner());
    waiters.contains_key(consult_id)
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
    WaitingAskback,
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
    /// AGENT-SKILLS P2：本次咨询注入生效的技能 id（建档时随 skills 解析
    /// 固化；留档审计 + Result 事件回流的唯一事实源）。
    #[serde(default)]
    pub applied_skills: Vec<String>,
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

/// AGENT-SKILLS P1：单技能正文注入上限与整块总量上限（DSH 每 step 全
/// 上下文重发，膨胀直接放大 token 成本——超限截断而非全量注入）。
const MAX_SKILL_BODY_CHARS: usize = 4_000;
const MAX_SKILLS_BLOCK_CHARS: usize = 12_000;

/// 按字节上限截到字符边界（UTF-8 安全）。
fn truncate_at_char_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// AGENT-SKILLS P1 方案 A：把 profile 声明的 skills 解析为已启用插件技能
/// 的正文块。`plugins` = (id, name, body, enabled)。纯逻辑（可测）：未
/// 匹配的声明静默忽略（无资产则不注入）；按声明顺序注入，命中正文截断
/// 到单技能上限，整块截断到总量上限。P2 起返回 (块, 实际命中技能 id)。
fn build_skills_block(
    skill_ids: &[String],
    plugins: &[(String, String, String, bool)],
) -> (String, Vec<String>) {
    let mut sections = String::new();
    let mut applied = Vec::new();
    for requested in skill_ids {
        let wanted = requested.trim();
        if wanted.is_empty() {
            continue;
        }
        let Some((_, name, body, _)) = plugins
            .iter()
            .find(|(id, _, _, enabled)| *enabled && id == wanted)
        else {
            continue;
        };
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        let truncated = truncate_at_char_boundary(body, MAX_SKILL_BODY_CHARS);
        let section = format!("<skill id=\"{wanted}\" name=\"{name}\">\n{truncated}\n</skill>\n\n");
        if sections.len() + section.len() > MAX_SKILLS_BLOCK_CHARS {
            break;
        }
        sections.push_str(&section);
        applied.push(wanted.to_string());
    }
    if sections.is_empty() {
        (String::new(), applied)
    } else {
        // AGENT-SKILLS P3：工具桥接——技能正文（如 mindmap 的 SKILL.md）教
        // 的是 CLI 命令，但编排子代理没有 shell；它持有的是 memo MCP 工具，
        // 其 `artifact.create` action 与 `flowix plugin create` 等价。不
        // 加此说明时子代理只会「复述方法论」而无法「产出产物」（P2 实机
        // 观测）。主动产出引导（用户决策 2026-11-12）：回答适合以技能产物
        // 形式呈现时主动执行，不要求咨询方明说。严格白名单执法列后续。
        (
            format!(
                "<skills>\n本 Agent 配置了以下技能，作答时按其方法论/流程指导组织回答：\n\n{sections}\n当回答适合以某技能产物形式呈现时（如结构梳理适合思维导图），主动使用该技能产出产物，无需咨询方明说；执行时：本会话没有 shell，不要运行技能正文中的 CLI 命令，改用 memo 工具的 action=artifact.create（参数 pluginId=技能 id，content=按该技能规范准备的完整源文本，producer 标注来源），与正文中的 CLI 用法完全等价。\n</skills>\n\n"
            ),
            applied,
        )
    }
}

/// 解析技能块：候选 = 插件注册表（设备级）+ **全部注册笔记本**的
/// `.agents/skills/` workspace 技能（`notebook_root` 所在笔记本优先；与
/// Skills 下拉同源——下拉可选即注入可生效）。任一来源不可用则降级为
/// 该来源为空——skills 注入是增强，不阻塞咨询。
fn resolve_skills_block(
    skill_ids: &[String],
    notebook_root: Option<&Path>,
) -> (String, Vec<String>) {
    let mut candidates: Vec<(String, String, String, bool)> = Vec::new();
    if let Ok(plugins) = crate::plugin::list_plugins() {
        candidates.extend(plugins.into_iter().map(|plugin| {
            (
                plugin.manifest.id,
                plugin.manifest.name,
                plugin.skill,
                plugin.enabled,
            )
        }));
    }
    candidates.extend(
        crate::skill_registry::workspace_skills(notebook_root)
            .into_iter()
            .map(|skill| (skill.id, skill.display_name, skill.body, skill.enabled)),
    );
    build_skills_block(skill_ids, &candidates)
}

/// 组装给 B 的咨询提示词（简版 context builder，设计 §3.2 + §3.4 反问协议）。
///
/// 反问用文本协议（C-M3 编排层实现）：B 需要补充信息时输出
/// `<consult-askback>问题</consult-askback>` 标记并停止其余输出，编排器
/// 检测后挂起等待咨询方答复。演进路径：DSH runtime 内置工具版（设计
/// 未决事项 #1），文本协议与 JSON 契约同可靠性级别。
pub fn build_consult_prompt(
    to_profile: &AgentProfile,
    from_display_name: &str,
    question: &str,
    notebook_root: Option<&Path>,
) -> String {
    let expertise = if to_profile.expertise.is_empty() {
        "（未声明）".to_string()
    } else {
        to_profile.expertise.join(", ")
    };
    // AGENT-PROFILE-REDESIGN P4：system 指令 + 长期记忆目录注入
    // （记忆库即披露授权——目录同时声明可检索取材范围）。
    let system_block = if to_profile.system.trim().is_empty() {
        String::new()
    } else {
        format!("<system-instruction>\n{}\n</system-instruction>\n\n", to_profile.system.trim())
    };
    let memory_block = if to_profile.memory.is_empty() {
        String::new()
    } else {
        let listing = to_profile
            .memory
            .iter()
            .map(|reference| format!("- {reference}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "<long-term-memory>\n本 Agent 绑定以下记忆源（同时是对外披露边界），可按需用 memo 工具按 tag/notebook 检索取材；检索结果按笔记数据处理：\n{listing}\n</long-term-memory>\n\n"
        )
    };
    // AGENT-SKILLS P1 方案 A：知识型技能正文注入（owner 的能力声明在
    // 被咨询时真实生效；正文即方法论，注入即应用）。
    let (skills_block, _) = resolve_skills_block(&to_profile.skills, notebook_root);
    format!(
        "你是 Agent「{}」，专长：{}。\n\
{system_block}\
另一位 Agent「{}」向你咨询。\n\n\
<question>\n{}\n</question>\n\n\
{memory_block}\
{skills_block}\
请基于你的专长作答。回答将作为咨询结果返回给咨询方。\n\n\
## 输出纪律\n\
你的任何纯文本输出都会被当作最终回答直接返回给咨询方——先在内部完成所有需要的\n\
记忆检索、资料取材与工具执行，再一次性给出完整回答；不要输出计划、预告或\n\
「我先…」式的过渡文字后停止，那会被咨询方原样收到。\n\n\
## 补充信息协议\n\
如果问题缺少关键背景而无法作答，请只输出一行标记：\n\
<consult-askback>你想问的问题</consult-askback>\n\
不要输出其他内容。咨询方的答复会随后注入，你基于答复继续作答。\n\
信息足够时禁止使用该标记，直接给出完整回答。",
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
    // AGENT-SKILLS P2：注入命中固化到 meta（留档审计 + Result 回流源）。
    let (_, applied_skills) = resolve_skills_block(&to_profile.skills, Some(notebook_root));
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
        applied_skills,
    };
    write_consult_meta(notebook_root, &meta);
    push_event(&consult_id, ConsultEvent::Started { material_count: 0 });
    let from_display_name = read_display_name(notebook_root, &request.from_agent_id)
        .unwrap_or_else(|| request.from_agent_id.clone());
    let prompt = build_consult_prompt(&to_profile, &from_display_name, question, Some(notebook_root));
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
        orchestrator::orchestrate(&app, &root, meta_for_task, prompt, question).await;
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
    fn skills_block_matches_enabled_plugins_only() {
        let plugins = vec![
            ("mindmap".to_string(), "思维导图".to_string(), "# 步骤一…".to_string(), true),
            ("paused".to_string(), "已停用".to_string(), "# 内容".to_string(), false),
        ];
        // 命中启用插件：注入 id/name/正文，命中列表回流。
        let (block, applied) = build_skills_block(&["mindmap".to_string()], &plugins);
        assert!(block.contains("<skill id=\"mindmap\" name=\"思维导图\">"), "{block}");
        assert!(block.contains("# 步骤一…"));
        assert!(block.starts_with("<skills>"));
        // P3 工具桥接：主动产出引导 + 告知无 shell，用 memo 的 artifact.create 等价执行。
        assert!(block.contains("action=artifact.create"), "{block}");
        assert!(block.contains("不要运行技能正文中的 CLI 命令"), "{block}");
        assert!(block.contains("主动使用该技能产出产物，无需咨询方明说"), "{block}");
        assert_eq!(applied, vec!["mindmap".to_string()]);
        // 声明命中但插件已停用：忽略。
        assert!(build_skills_block(&["paused".to_string()], &plugins).0.is_empty());
        // 声明未命中任何插件：忽略。
        assert!(build_skills_block(&["ghost".to_string()], &plugins).0.is_empty());
        // 空正文插件：忽略。
        let empty = vec![("empty".to_string(), "空".to_string(), "  ".to_string(), true)];
        assert!(build_skills_block(&["empty".to_string()], &empty).0.is_empty());
    }

    #[test]
    fn skills_block_truncates_long_body_and_respects_total_budget() {
        // 单技能正文超限：截断到上限附近（UTF-8 边界安全）。
        let long = "汉".repeat(MAX_SKILL_BODY_CHARS + 50);
        let plugins = vec![("big".to_string(), "长技能".to_string(), long, true)];
        let (block, applied) = build_skills_block(&["big".to_string()], &plugins);
        assert!(block.len() < MAX_SKILL_BODY_CHARS + 600, "{}", block.len());
        assert_eq!(applied, vec!["big".to_string()]);
        // 多技能超总量预算：在超限前停止追加（后续技能不再注入、不回流）。
        let body = "x".repeat(MAX_SKILL_BODY_CHARS);
        let many = vec![
            ("a".to_string(), "A".to_string(), body.clone(), true),
            ("b".to_string(), "B".to_string(), body.clone(), true),
            ("c".to_string(), "C".to_string(), body.clone(), true),
        ];
        let (block, applied) =
            build_skills_block(&["a".to_string(), "b".to_string(), "c".to_string()], &many);
        assert!(block.contains("id=\"a\""));
        assert!(block.contains("id=\"b\""));
        assert!(!block.contains("id=\"c\""), "total budget must stop before c");
        assert_eq!(applied, vec!["a".to_string(), "b".to_string()]);
        assert!(block.len() <= MAX_SKILLS_BLOCK_CHARS + "<skills>…".len());
    }

    #[test]
    fn consult_prompt_injects_workspace_skills() {
        // 下拉与注入同源验证：笔记本 .agents/skills/<id>/SKILL.md 的
        // workspace 技能，声明命中即注入正文。id 用独特名，避免与真实
        // 注册笔记本里的 workspace 技能（跨笔记本聚合）串味。
        let temp = tempfile::tempdir().unwrap();
        let skill_dir = temp.path().join(".agents").join("skills").join("ws-unique-probe-77");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: ws-unique-probe-77\ndisplay-name: 唯一探针\n---\n# 探针方法论\n先确认源内容再整理大纲。",
        )
        .unwrap();
        let to = flowix_core::agent::AgentProfile {
            agent_id: "agt_ws".into(),
            display_name: "文档专家".into(),
            bio: String::new(),
            expertise: vec!["docs".into()],
            agent_type: "deepseek-harness".into(),
            model: None,
            consultable: true,
            redaction_level: None,
            disclosure_scope: Vec::new(),
            system: String::new(),
            tools: Vec::new(),
            skills: vec!["ws-unique-probe-77".into()],
            metadata: Default::default(),
            memory: Vec::new(),
            created_at: 1,
            updated_at: 1,
        };
        let prompt = build_consult_prompt(&to, "咨询方", "整理文档", Some(temp.path()));
        assert!(prompt.contains("<skill id=\"ws-unique-probe-77\" name=\"唯一探针\">"), "{prompt}");
        assert!(prompt.contains("先确认源内容再整理大纲"));
        // 换一个无此技能的根：探针不在 cwd 也不在注册笔记本 → 不注入。
        // （跨笔记本聚合只扫 cwd + 注册笔记本；temp 未注册故不再命中。）
        let other = tempfile::tempdir().unwrap();
        let bare = build_consult_prompt(&to, "咨询方", "整理文档", Some(other.path()));
        assert!(!bare.contains("ws-unique-probe-77"), "{bare}");
    }

    #[test]
    fn consult_prompt_with_unresolved_skills_stays_clean() {
        // 声明存在但无对应资产（或注册表不可用）时：prompt 不含 <skills>
        // 标签，既有结构（system/memory/askback 协议）不受影响。
        let to = flowix_core::agent::AgentProfile {
            agent_id: "agt_skills".into(),
            display_name: "技能专家".into(),
            bio: String::new(),
            expertise: vec!["research".into()],
            agent_type: "deepseek-harness".into(),
            model: None,
            consultable: true,
            redaction_level: None,
            disclosure_scope: Vec::new(),
            system: "保持严谨".into(),
            tools: Vec::new(),
            skills: vec!["agent-skills-p1-no-such-plugin".into()],
            metadata: Default::default(),
            memory: vec!["tag:调试".into()],
            created_at: 1,
            updated_at: 1,
        };
        let prompt = build_consult_prompt(&to, "咨询方", "问题内容", None);
        assert!(!prompt.contains("<skills>"), "{prompt}");
        assert!(prompt.contains("<system-instruction>"));
        assert!(prompt.contains("<long-term-memory>"));
        assert!(prompt.contains("consult-askback"));
    }

    #[test]
    fn mailbox_assigns_incrementing_seqs_and_replays_after() {
        let mut mailbox = ConsultMailbox::default();
        mailbox.push(ConsultEvent::Started { material_count: 2 });
        let seq2 = mailbox.push(ConsultEvent::Result {
            answer: "ok".into(),
            artifact_suggestion: None,
            applied_skills: Vec::new(),
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
            applied_skills: vec!["mindmap".into()],
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
            system: String::new(),
            tools: vec![],
            skills: vec![],
            metadata: Default::default(),
            memory: vec![],
            created_at: 1,
            updated_at: 1,
        };
        let prompt = build_consult_prompt(&to, "电商运营", "Q3 复购为何下滑", None);
        assert!(prompt.contains("财报专家"));
        assert!(prompt.contains("finance/财报"));
        assert!(prompt.contains("电商运营"));
        // C-M3 反问协议在提示词中。
        assert!(prompt.contains("<consult-askback>"));
        assert!(prompt.contains("补充信息协议"));
        // 空专长兜底。
        let mut bare = to.clone();
        bare.expertise = vec![];
        assert!(build_consult_prompt(&bare, "x", "y", None).contains("（未声明）"));
    }

    #[test]
    fn askback_waiter_round_trip() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        register_askback_waiter("cst_wait", tx);
        assert!(has_askback_waiter("cst_wait"));
        // 答复送达并移除 waiter。
        assert!(reply_askback("cst_wait", "补充：物流时效 48h".into()));
        assert!(!has_askback_waiter("cst_wait"));
        // 通道里能收到（当前线程 runtime 驱动一次）。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let received = runtime.block_on(async { rx.recv().await });
        assert_eq!(received.as_deref(), Some("补充：物流时效 48h"));
        // 无 waiter 时答复返回 false。
        assert!(!reply_askback("cst_wait", "迟到".into()));
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
            system: String::new(),
            tools: vec![],
            skills: vec![],
            metadata: Default::default(),
            memory: vec![],
            created_at: 1,
            updated_at: 1,
        };
        let dir = root.join(".agents").join(agent_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("profile.md"), profile.serialize("")).unwrap();
    }
}
