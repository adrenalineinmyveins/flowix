//! F6 脱敏管线编排（REDACTION-PIPELINE.md §3/§4）。
//!
//! - L0 载荷组装器：只让白名单 scope 内的材料进入管线（行级门控复用
//!   F2 `should_accept_event_in_notebook` 的同一语义：Notebook/Folder/Tag
//!   三种条目 + `.agents/`、`.flowix/` 硬拒），越界内容构造期丢弃并计入报告。
//! - L2 蒸馏：三段提示词（等级定义 / L1 文本 / 输出契约，占位符保留），
//!   通过注入的 distill 闭包执行（生产走 DSH agent 会话，测试走 fake）。
//! - L3 复扫：`flowix-redaction` 的 scan_with_map + 占位符完整性双向对账，
//!   命中即回炉（上限 2 次），超限报 `RULE_HIT_AFTER_DISTILL` /
//!   `PLACEHOLDER_BROKEN`，不降级直发。
//!
//! R3 级蒸馏失败可走纯规则兜底（`r3_fallback`：截断 + 首句提取）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use flowix_redaction::{
    self as l1, check_placeholder_integrity, CustomWord, PlaceholderProblem, RedactionOptions,
    RedactionReport,
};

use crate::config::access::AgentAccessEntry;
use crate::nurture::should_accept_event_in_notebook;

pub const REDACTION_LEVELS: &[&str] = &["r1", "r2", "r3"];
/// L3 复扫回炉上限（初次蒸馏之外最多重写次数），对应规格 §3.4「上限 2 次」。
pub const MAX_DISTILL_RETRIES: u32 = 2;
const R3_FALLBACK_MAX_CHARS: usize = 800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionPipelineError {
    ScopeEmpty,
    PayloadTooLarge { limit_kb: u32, actual_kb: u32 },
    InvalidLevel(String),
    DistillFailed(String),
    RuleHitAfterDistill { rule_ids: Vec<String>, attempts: u32 },
    PlaceholderBroken { problems: Vec<String>, attempts: u32 },
}

impl RedactionPipelineError {
    /// 稳定机器码（CLI 风格小写下划线），前端经 error-message.ts 归一展示。
    pub fn error_code(&self) -> String {
        match self {
            Self::ScopeEmpty => "scope_empty".to_string(),
            Self::PayloadTooLarge { .. } => "payload_too_large".to_string(),
            Self::InvalidLevel(_) => "invalid_level".to_string(),
            Self::DistillFailed(_) => "distill_failed".to_string(),
            Self::RuleHitAfterDistill { .. } => "rule_hit_after_distill".to_string(),
            Self::PlaceholderBroken { .. } => "placeholder_broken".to_string(),
        }
    }

    /// IPC 错误串：稳定机器码 + 冒号细节（前端按首个 `:` 前缀归一展示）。
    pub fn to_ipc_error(&self) -> String {
        match self {
            Self::DistillFailed(reason) => format!("distill_failed:{reason}"),
            Self::RuleHitAfterDistill { rule_ids, .. } => {
                format!("rule_hit_after_distill:{}", rule_ids.join(","))
            }
            Self::PlaceholderBroken { problems, .. } => {
                format!("placeholder_broken:{}", problems.join(";"))
            }
            Self::PayloadTooLarge { limit_kb, .. } => format!("payload_too_large:{limit_kb}"),
            Self::InvalidLevel(level) => format!("invalid_level:{level}"),
            Self::ScopeEmpty => self.error_code(),
        }
    }
}

// ---------------------------------------------------------------------------
// L0 载荷组装器
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L0Material {
    pub relative_path: String,
    pub tags: Vec<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L0Payload {
    /// question（围栏）+ 白名单内材料（围栏），进入 L1 的完整候选载荷。
    pub text: String,
    pub included_paths: Vec<String>,
    pub dropped_paths: Vec<String>,
    pub truncated: bool,
}

/// 大小写不敏感地把闭合围栏破坏串中和为单书名号形式（视觉可辨、语法失效），
/// 防止材料内容越权终结围栏。返回 (中和后文本, 是否发生了中和)。
pub fn sanitize_fence_breakers(text: &str) -> (String, bool) {
    let lower = text.to_ascii_lowercase();
    let needle = "</material>";
    let mut out = String::with_capacity(text.len());
    let mut sanitized = false;
    let mut consumed = 0usize;
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find(needle) {
        let start = search_from + rel;
        out.push_str(&text[consumed..start]);
        out.push_str("‹/material›");
        consumed = start + needle.len();
        search_from = consumed;
        sanitized = true;
    }
    out.push_str(&text[consumed..]);
    (out, sanitized)
}

/// L0 主函数：组装进入 L1 的候选载荷。
///
/// - enabled scope 为空，或过滤后无任何材料命中（交集空）→ `SCOPE_EMPTY`
/// - question 自身超限 → `PAYLOAD_TOO_LARGE`（材料按累计字节截断，不报错）
/// - 含闭合围栏破坏串的材料整体跳过（防注入，与 F3 渲染纪律一致），
///   question 中的破坏串就地中和
pub fn assemble_l0_payload(
    notebook_root: Option<&str>,
    question: &str,
    scopes: &[AgentAccessEntry],
    materials: Vec<L0Material>,
    limit_kb: usize,
) -> Result<L0Payload, RedactionPipelineError> {
    if !scopes.iter().any(|entry| entry.enabled) {
        return Err(RedactionPipelineError::ScopeEmpty);
    }
    let limit_bytes = limit_kb.saturating_mul(1024);
    let question_bytes = question.len();
    if question_bytes > limit_bytes {
        return Err(RedactionPipelineError::PayloadTooLarge {
            limit_kb: limit_kb as u32,
            actual_kb: (question_bytes as f32 / 1024.0).ceil() as u32,
        });
    }
    let (question_text, _) = sanitize_fence_breakers(question);

    let mut text = format!("<user-request>\n{question_text}\n</user-request>\n\n");
    let mut used = text.len();
    let mut included_paths = Vec::new();
    let mut dropped_paths = Vec::new();
    let mut truncated = false;

    for material in materials {
        if !should_accept_event_in_notebook(
            notebook_root,
            &material.relative_path,
            &material.tags,
            scopes,
        ) {
            dropped_paths.push(material.relative_path);
            continue;
        }
        if crate::plugin::contains_fence_breaker(&material.content) {
            dropped_paths.push(material.relative_path);
            continue;
        }
        let block = format!(
            "<material path=\"{}\" tags=\"{}\">\n{}\n</material>\n",
            material.relative_path,
            material.tags.join(", "),
            material.content
        );
        if used + block.len() > limit_bytes {
            truncated = true;
            break;
        }
        used += block.len();
        text.push_str(&block);
        included_paths.push(material.relative_path);
    }

    if included_paths.is_empty() {
        return Err(RedactionPipelineError::ScopeEmpty);
    }
    text = text.trim_end().to_string();
    Ok(L0Payload {
        text,
        included_paths,
        dropped_paths,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// L2 蒸馏（纯函数层）
// ---------------------------------------------------------------------------

fn level_spec(level: &str) -> Result<&'static str, RedactionPipelineError> {
    match level {
        "r1" => Ok(
            "R1 事实摘要：只保留结论与关键数字，去除所有叙事与背景。输出形如「近 90 天复购率 23%，主力 SKU 3 个」。",
        ),
        "r2" => Ok(
            "R2 泛化摘要：保留事件脉络但去实体化——公司、人名、产品名一律泛化为「某电商公司」「某客户」这类类别词，不保留可识别个体的表述。",
        ),
        "r3" => Ok(
            "R3 领域抽象：只提炼问题模式本身，形如「复购率下滑时如何归因物流环节？」，不包含任何具体事件与实体。",
        ),
        other => Err(RedactionPipelineError::InvalidLevel(other.to_string())),
    }
}

/// 蒸馏提示词：三段拼装（等级定义与改写规则 / L1 文本 / 输出契约）。
pub fn build_distill_prompt(l1_text: &str, target_level: &str) -> Result<String, RedactionPipelineError> {
    let spec = level_spec(target_level)?;
    let (material, _) = sanitize_fence_breakers(l1_text);
    Ok(format!(
        "你是脱敏蒸馏器。把 <material> 中的文本改写为目标等级的版本。\n\n\
## 目标等级\n{spec}\n\n\
## 待改写文本\n<material>\n{material}\n</material>\n\n\
## 输出契约\n\
1. 保留文本中所有形如 ⟦KIND-n⟧ 的占位符（如 ⟦PHONE-1⟧）：一个不能少、一个不能改、不新增、不展开还原其含义。\n\
2. 不引入任何新的具体人名、号码、邮箱、证件号、路径、URL 凭据或绝对日期时间；需要指代时用类别词（某公司、某客户、上周）。\n\
3. 输出为纯 Markdown 文本，只输出改写结果本身，不要解释、不要复述契约。\n\
4. 保留原文的核心信息量，篇幅不超过原文。"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistillCheckError {
    RuleHit { rule_id: String, count: usize },
    PlaceholderUnknown(String),
    PlaceholderMissing(String),
}

impl DistillCheckError {
    fn describe(&self) -> String {
        match self {
            Self::RuleHit { rule_id, count } => {
                format!("规则 {rule_id} 命中 {count} 处（敏感项被改写回来了）")
            }
            Self::PlaceholderUnknown(token) => {
                format!("出现映射表之外占位符 {token}（疑似拼凑）")
            }
            Self::PlaceholderMissing(token) => {
                format!("占位符 {token} 在输出中缺失（疑似被丢弃）")
            }
        }
    }
}

/// L3 复扫：对蒸馏稿重跑 L1 全部规则（豁免合法占位符）+ 占位符完整性双向对账。
/// 空 Vec = 通过。
pub fn validate_distilled(
    distilled: &str,
    placeholder_map: &BTreeMap<String, String>,
    options: &RedactionOptions,
    words: &[CustomWord],
) -> Vec<DistillCheckError> {
    let mut problems = Vec::new();
    for hit in l1::scan_with_map(distilled, options, words, placeholder_map) {
        if hit.rule_id == "placeholder" {
            continue;
        }
        problems.push(DistillCheckError::RuleHit {
            rule_id: hit.rule_id,
            count: hit.count,
        });
    }
    for problem in check_placeholder_integrity(distilled, placeholder_map) {
        problems.push(match problem {
            PlaceholderProblem::UnknownInText(token) => {
                DistillCheckError::PlaceholderUnknown(token)
            }
            PlaceholderProblem::MissingFromText(token) => {
                DistillCheckError::PlaceholderMissing(token)
            }
        });
    }
    problems
}

/// R3 级纯规则兜底：截断 + 首句提取（DISTILL_FAILED 时由调用方选用）。
/// 首句 = 首个中文/英文句读边界前的片段；整体截断到 R3_FALLBACK_MAX_CHARS。
pub fn r3_fallback(l1_text: &str) -> String {
    let body = l1_text.trim();
    if body.is_empty() {
        return String::new();
    }
    let first_sentence_end = body
        .char_indices()
        .find(|(_, ch)| matches!(ch, '。' | '！' | '？' | '!' | '?' | '\n' | ';'))
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(body.len());
    let head: String = body.chars().take(R3_FALLBACK_MAX_CHARS).collect();
    let sentence: String = body[..first_sentence_end.min(body.len())]
        .chars()
        .take(R3_FALLBACK_MAX_CHARS)
        .collect();
    if sentence.len() >= 8 {
        sentence
    } else {
        head
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillOutcome {
    pub text: String,
    pub attempts: u32,
    pub used_fallback: bool,
}

/// L2+L3 编排状态机：初次蒸馏 + 复扫回炉（上限 `MAX_DISTILL_RETRIES`）。
///
/// `distill` 收到完整提示词、返回蒸馏稿；任何调用失败立即传播
/// `DISTILL_FAILED`（不降级直发 L1 文本，交用户决定）。回炉提示词在
/// 基础提示词后附上次拦截清单。
pub async fn distill_with_recheck<F, Fut>(
    l1_text: &str,
    target_level: &str,
    placeholder_map: &BTreeMap<String, String>,
    options: &RedactionOptions,
    words: &[CustomWord],
    mut distill: F,
) -> Result<DistillOutcome, RedactionPipelineError>
where
    F: FnMut(&str) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let base_prompt = build_distill_prompt(l1_text, target_level)?;
    let mut prompt = base_prompt.clone();
    let mut attempts: u32 = 0;
    loop {
        let distilled = distill(&prompt)
            .await
            .map_err(RedactionPipelineError::DistillFailed)?;
        attempts += 1;
        let problems =
            validate_distilled(&distilled, placeholder_map, options, words);
        if problems.is_empty() {
            return Ok(DistillOutcome {
                text: distilled,
                attempts,
                used_fallback: false,
            });
        }
        if attempts > MAX_DISTILL_RETRIES {
            return Err(classify_problems(problems, attempts));
        }
        let listing = problems
            .iter()
            .map(DistillCheckError::describe)
            .collect::<Vec<_>>()
            .join("；");
        prompt = format!(
            "{base_prompt}\n\n## 上次输出被后校验拦截\n{listing}\n请重新改写，务必逐条修正以上问题。"
        );
    }
}

fn classify_problems(
    problems: Vec<DistillCheckError>,
    attempts: u32,
) -> RedactionPipelineError {
    let rule_ids: Vec<String> = problems
        .iter()
        .filter_map(|p| match p {
            DistillCheckError::RuleHit { rule_id, .. } => Some(rule_id.clone()),
            _ => None,
        })
        .collect();
    if !rule_ids.is_empty() {
        return RedactionPipelineError::RuleHitAfterDistill { rule_ids, attempts };
    }
    RedactionPipelineError::PlaceholderBroken {
        problems: problems.iter().map(DistillCheckError::describe).collect(),
        attempts,
    }
}

// ---------------------------------------------------------------------------
// DSH 接线与完整 preview 编排
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionPreviewReport {
    pub final_text: String,
    pub placeholder_map: BTreeMap<String, String>,
    pub l1_hits: Vec<l1::RuleHit>,
    pub attempts: u32,
    pub used_fallback: bool,
    pub included_paths: Vec<String>,
    pub dropped_paths: Vec<String>,
    pub truncated: bool,
    pub target_level: String,
    pub rule_set_version: String,
}

/// 完整 preview 管线：scope 解析 → L0 → L1 → L2（DSH 蒸馏 + L3 复扫回炉）。
///
/// R3 级蒸馏失败走 `r3_fallback`（used_fallback = true），其余等级的
/// `DISTILL_FAILED` 原样上抛。
pub async fn preview_full(
    app: &tauri::AppHandle,
    notebook_path: &str,
    question: &str,
    scope_entry_ids: &[String],
    target_level: &str,
) -> Result<RedactionPreviewReport, RedactionPipelineError> {
    use tauri::Manager;

    level_spec(target_level)?;
    let config = crate::commands::redaction::redaction_config_for_pipeline();
    let words = crate::commands::redaction::redaction_words_for_pipeline();

    let state = app.state::<crate::app::state::AppState>();
    let scopes: Vec<AgentAccessEntry> = state
        .agent_access
        .get_config()
        .entries
        .into_iter()
        .filter(|entry| scope_entry_ids.contains(&entry.id) && entry.enabled)
        .collect();

    let (notebook_id, notebook_root) = resolve_notebook(&state, notebook_path)?;
    let rows = {
        let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
        let tag_paths: Vec<String> = scopes
            .iter()
            .filter_map(|entry| entry.tag_path.clone())
            .collect();
        memo_file
            .list_note_materials_updated_after(&notebook_id, &tag_paths, 0, 200)
            .map_err(|e| {
                RedactionPipelineError::DistillFailed(format!("query materials: {e}"))
            })?
    };
    let materials: Vec<L0Material> = rows
        .into_iter()
        .map(|row| L0Material {
            relative_path: row.relative_path,
            tags: row.tags,
            content: String::new(),
        })
        .collect();
    let materials = hydrate_materials(&notebook_root, materials);

    let payload = assemble_l0_payload(
        Some(&notebook_root.to_string_lossy()),
        question,
        &scopes,
        materials,
        config.payload_limit_kb as usize,
    )?;

    let options = RedactionOptions {
        strict_luhn: config.strict_luhn,
        rewrite_absolute_time: config.rewrite_absolute_time,
        now: if config.rewrite_absolute_time {
            Some(chrono::Local::now().date_naive())
        } else {
            None
        },
    };
    let l1_report: RedactionReport = l1::redact(&payload.text, &options, &words);

    let root_for_distill = notebook_root.clone();
    let app_for_distill = app.clone();
    let distill = move |prompt: &str| {
        let app = app_for_distill.clone();
        let root = root_for_distill.clone();
        let prompt = prompt.to_string();
        async move {
            let state = app.state::<crate::app::state::AppState>();
            crate::nurture::run_distill_once(
                &app,
                &state,
                &root,
                &prompt,
                "deepseek-harness",
                "redaction:distill",
            )
            .await
            .map(|(answer, _)| answer)
        }
    };

    let outcome = match distill_with_recheck(
        &l1_report.text,
        target_level,
        &l1_report.placeholder_map,
        &options,
        &words,
        distill,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(RedactionPipelineError::DistillFailed(reason)) if target_level == "r3" => {
            DistillOutcome {
                text: r3_fallback(&l1_report.text),
                attempts: 0,
                used_fallback: true,
            }
            .tap_debug(&reason)
        }
        Err(error) => return Err(error),
    };

    Ok(RedactionPreviewReport {
        final_text: outcome.text,
        placeholder_map: l1_report.placeholder_map,
        l1_hits: l1_report.hits,
        attempts: outcome.attempts,
        used_fallback: outcome.used_fallback,
        included_paths: payload.included_paths,
        dropped_paths: payload.dropped_paths,
        truncated: payload.truncated,
        target_level: target_level.to_string(),
        rule_set_version: l1::RULE_CATALOG_VERSION.to_string(),
    })
}

trait TapDebug {
    fn tap_debug(self, reason: &str) -> Self;
}

impl TapDebug for DistillOutcome {
    fn tap_debug(self, reason: &str) -> Self {
        tracing::warn!(reason, "r3 distill failed, using rule fallback");
        self
    }
}

fn resolve_notebook(
    state: &crate::app::state::AppState,
    notebook_path: &str,
) -> Result<(String, PathBuf), RedactionPipelineError> {
    let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
    let canonical = dunce::canonicalize(notebook_path)
        .map_err(|e| RedactionPipelineError::DistillFailed(format!("notebook path: {e}")))?;
    let config = memo_file
        .read_notebook_configs()
        .map_err(|e| RedactionPipelineError::DistillFailed(format!("read configs: {e}")))?
        .into_iter()
        .find(|config| {
            dunce::canonicalize(&config.path)
                .map(|path| path == canonical)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            RedactionPipelineError::DistillFailed("notebook is not registered".into())
        })?;
    Ok((config.id, PathBuf::from(config.path)))
}

fn hydrate_materials(root: &std::path::Path, materials: Vec<L0Material>) -> Vec<L0Material> {
    materials
        .into_iter()
        .filter_map(|mut material| {
            let path = flowix_core::memo_file::notebook_path_from_relative(root, &material.relative_path)
                .ok()?;
            let content = std::fs::read_to_string(path).ok()?;
            material.content = flowix_core::memo_file::extract_body_content(&content).to_string();
            Some(material)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::access::{AgentAccessConfig, AgentAccessEntry, AgentAccessKind};

    fn now() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2025, 3, 21).unwrap()
    }

    fn opts() -> RedactionOptions {
        RedactionOptions {
            strict_luhn: false,
            rewrite_absolute_time: true,
            now: Some(now()),
        }
    }

    fn entry(kind: AgentAccessKind, path: &str, tag_path: Option<&str>) -> AgentAccessEntry {
        AgentAccessEntry {
            id: format!("e-{path}-{kind:?}"),
            kind,
            path: path.to_string(),
            name: path.to_string(),
            enabled: true,
            tag_path: tag_path.map(str::to_string),
            workspace: false,
            added_at: 0,
            updated_at: 0,
            missing: false,
        }
    }

    fn material(path: &str, tags: &[&str], marker: &str) -> L0Material {
        L0Material {
            relative_path: path.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            content: format!("内容标记 {marker}"),
        }
    }

    // ---------------- L0 ----------------

    #[test]
    fn l0_scope_empty_rejected() {
        let err = assemble_l0_payload(None, "q", &[], vec![material("a.md", &[], "m")], 64);
        assert_eq!(err.unwrap_err(), RedactionPipelineError::ScopeEmpty);

        let disabled = AgentAccessEntry {
            enabled: false,
            ..entry(AgentAccessKind::Notebook, "/nb", None)
        };
        let err = assemble_l0_payload(None, "q", &[disabled], vec![material("a.md", &[], "m")], 64);
        assert_eq!(err.unwrap_err(), RedactionPipelineError::ScopeEmpty);
    }

    #[test]
    fn l0_intersection_empty_rejected() {
        let scopes = vec![entry(AgentAccessKind::Tag, "", Some("工作/项目A"))];
        let err = assemble_l0_payload(
            None,
            "q",
            &scopes,
            vec![material("a.md", &["生活"], "m")],
            64,
        );
        assert_eq!(err.unwrap_err(), RedactionPipelineError::ScopeEmpty);
    }

    #[test]
    fn l0_three_scope_kinds_and_drop() {
        let root = "/nb";
        let scopes = vec![
            entry(AgentAccessKind::Folder, "/nb/日记", None),
            entry(AgentAccessKind::Folder, "/nb/工作", None),
            entry(AgentAccessKind::Tag, "", Some("工作")),
        ];
        let materials = vec![
            material("工作/x.md", &["工作/项目A"], "M1"),
            material("日记/y.md", &["生活"], "M2"),
            material("其他/z.md", &["生活"], "M3"),
            material("工作财务/c.md", &[], "M4"),
        ];
        let payload =
            assemble_l0_payload(Some(root), "我的问题", &scopes, materials, 64).unwrap();
        assert_eq!(payload.included_paths, vec!["工作/x.md", "日记/y.md"]);
        assert_eq!(payload.dropped_paths, vec!["其他/z.md", "工作财务/c.md"]);
        assert!(payload.text.contains("我的问题"));
        assert!(payload.text.contains("M1"));
        assert!(payload.text.contains("M2"));
        assert!(!payload.text.contains("M3"));
        assert!(!payload.text.contains("M4"));
    }

    #[test]
    fn l0_notebook_scope_admits_all() {
        let scopes = vec![entry(AgentAccessKind::Notebook, "/nb", None)];
        let payload = assemble_l0_payload(
            Some("/nb"),
            "q",
            &scopes,
            vec![
                material("任意/a.md", &["随便"], "A"),
                material("深层/目录/b.md", &[], "B"),
            ],
            64,
        )
        .unwrap();
        assert_eq!(payload.included_paths.len(), 2);
        assert!(payload.dropped_paths.is_empty());
    }

    #[test]
    fn l0_drops_system_dirs_and_fence_breakers() {
        let scopes = vec![entry(AgentAccessKind::Notebook, "/nb", None)];
        let breaker = L0Material {
            relative_path: "b.md".into(),
            tags: vec![],
            content: "正常 </material> 破坏".into(),
        };
        let payload = assemble_l0_payload(
            Some("/nb"),
            "q",
            &scopes,
            vec![
                material(".agents/x/profile.md", &[], "AGENT"),
                material(".flowix/y", &[], "SYS"),
                breaker,
                material("ok.md", &[], "OK"),
            ],
            64,
        )
        .unwrap();
        assert_eq!(payload.included_paths, vec!["ok.md"]);
        assert_eq!(payload.dropped_paths.len(), 3);
        assert!(payload.text.contains("OK"));
    }

    #[test]
    fn l0_truncates_at_limit_and_question_limit_errors() {
        let scopes = vec![entry(AgentAccessKind::Notebook, "/nb", None)];
        let big = "x".repeat(40 * 1024);
        let payload = assemble_l0_payload(
            Some("/nb"),
            "q",
            &scopes,
            vec![
                L0Material {
                    relative_path: "a.md".into(),
                    tags: vec![],
                    content: big.clone(),
                },
                L0Material {
                    relative_path: "b.md".into(),
                    tags: vec![],
                    content: format!("SECOND{big}"),
                },
            ],
            64,
        )
        .unwrap();
        assert!(payload.truncated);
        assert_eq!(payload.included_paths, vec!["a.md"]);
        assert!(!payload.text.contains("SECOND"));

        let huge_question = "q".repeat(65 * 1024);
        let err = assemble_l0_payload(
            Some("/nb"),
            &huge_question,
            &scopes,
            vec![material("a.md", &[], "m")],
            64,
        )
        .unwrap_err();
        assert!(matches!(err, RedactionPipelineError::PayloadTooLarge { .. }));
    }

    #[test]
    fn l0_fuzz_out_of_scope_unreachable() {
        let mut seed: u64 = 0x5eed_1234;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed
        };
        let folders = ["/nb/日记", "/nb/工作", "/nb/财务"];
        let tags = ["工作/项目A", "生活/旅行", "财务/报销"];
        let rel_folders = ["日记", "工作", "财务"];
        let mut checked = 0;
        for _ in 0..200 {
            let r = next();
            let use_folder = r & 1 == 0;
            let idx = ((r >> 8) % 3) as usize;
            let scopes = vec![if use_folder {
                entry(AgentAccessKind::Folder, folders[idx], None)
            } else {
                entry(AgentAccessKind::Tag, "", Some(tags[idx]))
            }];
            let in_scope_path = if use_folder {
                format!("{}/f{}.md", rel_folders[idx], r % 7)
            } else {
                format!("note{}.md", r % 7)
            };
            let in_scope_tags: Vec<String> = if use_folder {
                vec![]
            } else {
                let mut t = tags[idx].to_string();
                if r & 2 == 0 {
                    t.push_str("/子任务");
                }
                vec![t]
            };
            let out_idx = ((r >> 16) % 2) as usize;
            let out_scope = if use_folder {
                let other = (idx + 1 + out_idx) % 3;
                vec![material(
                    &format!("{}/leak{}.md", rel_folders[other], r % 5),
                    &[],
                    "LEAK_MARKER",
                )]
            } else {
                let other = (idx + 1 + out_idx) % 3;
                vec![material(
                    &format!("leak{}.md", r % 5),
                    &[tags[other]],
                    "LEAK_MARKER",
                )]
            };
            let mut materials = vec![L0Material {
                relative_path: in_scope_path,
                tags: in_scope_tags,
                content: "SAFE_MARKER".to_string(),
            }];
            materials.extend(out_scope);
            let result = assemble_l0_payload(Some("/nb"), "q", &scopes, materials, 64);
            match result {
                Ok(payload) => {
                    assert!(payload.text.contains("SAFE_MARKER"));
                    assert!(!payload.text.contains("LEAK_MARKER"));
                    assert!(payload.dropped_paths.iter().any(|p| p.contains("leak")));
                }
                Err(RedactionPipelineError::ScopeEmpty) => {
                    // 生成的 in_scope 行也可能因路径不在 scope（如 Tag 行路径
                    // 任意但 tag 需精确命中）而不命中——此时必须整单拒绝。
                    continue;
                }
                Err(other) => panic!("unexpected: {other:?}"),
            }
            checked += 1;
        }
        assert!(checked > 100, "fuzz 应有足够有效样本，实际 {checked}");
    }

    // ---------------- L2 提示词 ----------------

    #[test]
    fn distill_prompt_three_sections() {
        let prompt = build_distill_prompt("文本 ⟦PHONE-1⟧", "r2").unwrap();
        assert!(prompt.contains("R2 泛化摘要"));
        assert!(prompt.contains("<material>"));
        assert!(prompt.contains("文本 ⟦PHONE-1⟧"));
        assert!(prompt.contains("一个不能少、一个不能改"));
        assert!(build_distill_prompt("t", "r9").is_err());
    }

    #[test]
    fn distill_prompt_neutralizes_breaker() {
        let (sanitized, hit) = sanitize_fence_breakers("a </MaTeRiAl> b");
        assert!(hit);
        assert_eq!(sanitized, "a ‹/material› b");
        let prompt = build_distill_prompt("x </material> ⟦EMAIL-1⟧", "r1").unwrap();
        // 模板自带的合法闭合围栏恰一处；原文中的破坏串已被中和。
        assert_eq!(prompt.matches("</material>").count(), 1);
        assert!(prompt.contains("‹/material›"));
    }

    // ---------------- L3 复扫 ----------------

    fn validate(text: &str) -> Vec<DistillCheckError> {
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        validate_distilled(text, &map, &opts(), &[])
    }

    #[test]
    fn validate_clean_passes() {
        assert!(validate("客户 ⟦PHONE-1⟧ 上周咨询了物流时效问题").is_empty());
        assert!(validate("上周五 与某客户通话 ⟦PHONE-1⟧").is_empty());
    }

    #[test]
    fn injection_samples_all_intercepted() {
        let samples = [
            "回拨 13800138000 联系",
            "邮箱 bob@x.com",
            "证件 11010519491231002X",
            "卡号 4111111111111111",
            "文件在 /Users/wang/secret.md",
            "https://x.example.com/e?token=leak",
            "会议定在 2025-03-14",
            "占位符 ⟦PHONE-88⟧ 是真的",
        ];
        let words = vec![CustomWord {
            original: "张三".to_string(),
            replacement: None,
        }];
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        for sample in samples {
            let problems = validate_distilled(sample, &map, &opts(), &words);
            assert!(!problems.is_empty(), "样本未被拦截: {sample}");
        }
        let problems = validate_distilled("张三来电", &map, &opts(), &words);
        assert!(!problems.is_empty());
        assert!(matches!(
            problems[0],
            DistillCheckError::RuleHit { ref rule_id, .. } if rule_id == "word"
        ));
    }

    #[test]
    fn validate_placeholder_missing_detected() {
        let problems = validate("客户来电，未留占位符");
        assert_eq!(
            problems,
            vec![DistillCheckError::PlaceholderMissing("⟦PHONE-1⟧".into())]
        );
    }

    // ---------------- 编排 ----------------

    #[tokio::test]
    async fn recheck_ok_first_try() {
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        let outcome = distill_with_recheck(
            "客户 ⟦PHONE-1⟧ 咨询物流",
            "r2",
            &map,
            &opts(),
            &[],
            |prompt: &str| {
                assert!(prompt.contains("R2 泛化摘要"));
                async { Ok("某客户 ⟦PHONE-1⟧ 咨询物流时效".to_string()) }
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.attempts, 1);
        assert!(!outcome.used_fallback);
    }

    #[tokio::test]
    async fn recheck_retry_then_pass() {
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        let mut calls = 0;
        let outcome = distill_with_recheck(
            "客户 ⟦PHONE-1⟧ 咨询",
            "r2",
            &map,
            &opts(),
            &[],
            |prompt: &str| {
                calls += 1;
                let prompt = prompt.to_string();
                async move {
                    if calls == 1 {
                        Ok("客户 13800138000 咨询".to_string())
                    } else {
                        assert!(prompt.contains("上次输出被后校验拦截"));
                        assert!(prompt.contains("phone"));
                        Ok("某客户 ⟦PHONE-1⟧ 咨询".to_string())
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.attempts, 2);
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn recheck_exhausted_reports_rule_hit() {
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        let err = distill_with_recheck(
            "t ⟦PHONE-1⟧",
            "r1",
            &map,
            &opts(),
            &[],
            |_| async { Ok("回拨 13800138000".to_string()) },
        )
        .await
        .unwrap_err();
        match err {
            RedactionPipelineError::RuleHitAfterDistill { rule_ids, attempts } => {
                assert!(rule_ids.contains(&"phone".to_string()));
                assert_eq!(attempts, MAX_DISTILL_RETRIES + 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn recheck_placeholder_broken_classified() {
        let mut map = BTreeMap::new();
        map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
        let err = distill_with_recheck(
            "t ⟦PHONE-1⟧",
            "r1",
            &map,
            &opts(),
            &[],
            |_| async { Ok("占位符 ⟦PHONE-77⟧ 出现".to_string()) },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            RedactionPipelineError::PlaceholderBroken { .. }
        ));
        assert_eq!(err.error_code(), "placeholder_broken");
    }

    #[tokio::test]
    async fn recheck_distill_failure_propagates() {
        let err = distill_with_recheck("t", "r1", &BTreeMap::new(), &opts(), &[], |_| {
            async { Err("model down".to_string()) }
        })
        .await
        .unwrap_err();
        assert!(matches!(err, RedactionPipelineError::DistillFailed(_)));
        assert_eq!(err.error_code(), "distill_failed");
    }

    #[test]
    fn r3_fallback_first_sentence_and_truncation() {
        assert_eq!(r3_fallback("复购率下滑。具体细节省略。"), "复购率下滑。");
        let long: String = "很长的句子没有句读".repeat(200);
        let fallback = r3_fallback(&long);
        assert!(fallback.chars().count() <= R3_FALLBACK_MAX_CHARS);
        assert!(!fallback.is_empty());
        assert!(r3_fallback("   ").is_empty());
    }

    #[test]
    fn error_codes_stable() {
        assert_eq!(RedactionPipelineError::ScopeEmpty.error_code(), "scope_empty");
        assert_eq!(
            RedactionPipelineError::RuleHitAfterDistill {
                rule_ids: vec![],
                attempts: 3
            }
            .error_code(),
            "rule_hit_after_distill"
        );
    }
}
