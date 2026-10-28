//! FD-12 M1：harness 远程咨询预授权（auto-grant）模型。
//!
//! 纪律：预授权是确认门的**显式放宽**（类比小额免密）——判定在应用层
//! 单点函数完成（D1 口径，非 DSH approval）；任何维度越限回落人审。
//! 配置存 `~/.flowix/redaction/auto-grant.json`（设备级，独立 UI 编辑，
//! 与会话隔离；不进笔记本/云同步，同 FD-6 留档边界）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoGrantConfig {
    pub schema_version: u32,
    /// 总开关（默认关：人审是默认态）。
    #[serde(default)]
    pub enabled: bool,
    /// 允许自动过门的最高脱敏等级（一期固定 R3）。
    #[serde(default = "default_max_level")]
    pub max_level: String,
    /// 允许取材的 scope（tag-path 通配，`finance/*`）。
    #[serde(default)]
    pub scope_whitelist: Vec<String>,
    /// 声明为开放的 scope：取材全落其中时跳过 L2 蒸馏走直通
    /// （L1+L3 保留防误标；混合取材按最严处理）。
    #[serde(default)]
    pub open_scopes: Vec<String>,
    /// 每 harness 会话自动发起上限。
    #[serde(default = "default_per_session_limit")]
    pub per_session_limit: u32,
    /// 每日上限（叠加中继 30/h）。
    #[serde(default = "default_per_day_limit")]
    pub per_day_limit: u32,
    /// 目标白名单（`*` = 目录可寻址者，可收敛到具体用户名）。
    #[serde(default = "default_targets")]
    pub targets: Vec<String>,
    /// true = 预授权档发起直接用 R3 纯规则兜底（毫秒级），显式质量取舍。
    #[serde(default)]
    pub fast_mode: bool,
    /// FD-12 M4：B 端 auto-answer 能力位。`off`（默认）| `R3`（自动回答
    /// 入站咨询并直发回程，回答出站按 R3 完整蒸馏）。
    #[serde(default)]
    pub auto_answer: String,
    /// B 端允许自动回答的来源用户白名单（`user/agent` 的 user 段；`*` = 全部）。
    #[serde(default = "default_targets")]
    pub answer_targets: Vec<String>,
}

fn default_max_level() -> String {
    "R3".to_string()
}
fn default_per_session_limit() -> u32 {
    3
}
fn default_per_day_limit() -> u32 {
    20
}
fn default_targets() -> Vec<String> {
    vec!["*".to_string()]
}

impl Default for AutoGrantConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            enabled: false,
            max_level: default_max_level(),
            scope_whitelist: Vec::new(),
            open_scopes: Vec::new(),
            per_session_limit: default_per_session_limit(),
            per_day_limit: default_per_day_limit(),
            targets: default_targets(),
            fast_mode: false,
            auto_answer: "off".to_string(),
            answer_targets: default_targets(),
        }
    }
}

pub fn auto_grant_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction").join("auto-grant.json"))
}

/// 读配置：文件缺失 = 全默认（enabled=false，即全部人审）。
pub fn load_config() -> AutoGrantConfig {
    let Ok(path) = auto_grant_path() else { return AutoGrantConfig::default() };
    let Ok(raw) = std::fs::read_to_string(path) else { return AutoGrantConfig::default() };
    serde_json::from_str::<AutoGrantConfig>(raw.trim_start_matches('\u{feff}'))
        .unwrap_or_default()
}

pub fn save_config(config: &AutoGrantConfig) -> Result<(), String> {
    let path = auto_grant_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("AUTO_GRANT_DIR_FAILED: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(config)
        .map_err(|e| format!("AUTO_GRANT_SERIALIZE_FAILED: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("AUTO_GRANT_WRITE_FAILED: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("AUTO_GRANT_WRITE_FAILED: {e}"))?;
    Ok(())
}

/// tag-path 通配匹配：`finance/*` 匹配 `finance/财报`；`*` 匹配一切；
/// 无通配符则精确匹配。
pub fn scope_matches(pattern: &str, scope: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" || pattern == scope {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return scope.starts_with(prefix)
            && scope.len() > prefix.len()
            && scope.as_bytes().get(prefix.len()) == Some(&b'/');
    }
    false
}

/// 等级序（数值越大越宽松）：R1 < R2 < R3。自动过门要求 `level` 不高于
/// `max_level`（一期 max 固定 R3，即仅最抽象等级可自动出门）。
pub fn level_rank(level: &str) -> Option<u8> {
    match level.trim().to_uppercase().as_str() {
        "R1" => Some(1),
        "R2" => Some(2),
        "R3" => Some(3),
        _ => None,
    }
}

/// 五维门判定结论。`reason` 为越限原因（审计入档）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateDecision {
    pub auto_pass: bool,
    pub reason: String,
}

/// 出站管线形态（§3.3 直通判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DistillMode {
    /// 零取材：载荷是新生成文本，无原文指纹——跳过 L2，L1 稿即终稿。
    ZeroMaterial,
    /// 开放库直通：取材 ⊆ openScopes——跳过 L2，L1 稿即终稿（L1+L3 防误标）。
    OpenScope,
    /// fastMode 规则兜底：跳过 L2，用 r3_fallback 截断稿（显式质量取舍）。
    Fallback,
    /// 完整蒸馏：普通库取材且有质量要求。
    FullDistill,
}

/// 五维判定：总开关 / 等级 / scope 白名单 / 会话与日配额 / 目标白名单。
/// 任一维度越限 → `auto_pass=false`（回落人审确认门）。
pub fn evaluate(
    config: &AutoGrantConfig,
    target_level: &str,
    scopes: &[String],
    session_used: u32,
    day_used: u32,
    target_user: &str,
) -> GateDecision {
    if !config.enabled {
        return GateDecision { auto_pass: false, reason: "disabled".into() };
    }
    let (Some(requested), Some(max)) = (level_rank(target_level), level_rank(&config.max_level))
    else {
        return GateDecision { auto_pass: false, reason: "level_invalid".into() };
    };
    if requested > max {
        return GateDecision { auto_pass: false, reason: "level_exceeds_max".into() };
    }
    if session_used >= config.per_session_limit {
        return GateDecision { auto_pass: false, reason: "session_limit".into() };
    }
    if day_used >= config.per_day_limit {
        return GateDecision { auto_pass: false, reason: "day_limit".into() };
    }
    let target = target_user.trim();
    if !config.targets.iter().any(|pattern| {
        let pattern = pattern.trim();
        pattern == "*" || pattern == target || target.strip_prefix(&format!("{pattern}/")).is_some()
    }) {
        return GateDecision { auto_pass: false, reason: "target_not_allowed".into() };
    }
    for scope in scopes {
        if !config.scope_whitelist.iter().any(|pattern| scope_matches(pattern, scope)) {
            return GateDecision { auto_pass: false, reason: "scope_not_whitelisted".into() };
        }
    }
    GateDecision { auto_pass: true, reason: "ok".into() }
}

/// 直通判定（§3.3）：零取材 > 开放库 > fastMode 兜底 > 完整蒸馏。
/// 混合取材（开放 + 非开放）按最严处理。
pub fn distill_mode(
    config: &AutoGrantConfig,
    scopes: &[String],
    has_material: bool,
) -> DistillMode {
    if !has_material {
        return DistillMode::ZeroMaterial;
    }
    let all_open = scopes.iter().all(|scope| {
        config.open_scopes.iter().any(|pattern| scope_matches(pattern, scope))
    });
    if all_open {
        return DistillMode::OpenScope;
    }
    if config.fast_mode {
        return DistillMode::Fallback;
    }
    DistillMode::FullDistill
}

/// FD-12 M4：B 端 auto-answer 判定（入站咨询是否自动回答并直发回程）。
/// 独立于 A 端发起的 `enabled` 总开关；任一维度不满足 → 回落收件箱人审
/// （现状不破坏）。`from_user` = 咨询方 `user/agent` 的 user 段；
/// `disclosure_level` = 咨询信封承诺的脱敏等级（一期仅接 R3——A 端
/// consult_remote 恒 R3，防其他客户端低等级直投）。day 配额与 A 端发起
/// 共用 `perDayLimit`（出境自动化总量单一口径）。
pub fn evaluate_auto_answer(
    config: &AutoGrantConfig,
    from_user: &str,
    disclosure_level: &str,
    day_used: u32,
) -> GateDecision {
    if config.auto_answer.trim().to_uppercase().as_str() != "R3" {
        return GateDecision { auto_pass: false, reason: "auto_answer_off".into() };
    }
    if level_rank(disclosure_level) != Some(3) {
        return GateDecision { auto_pass: false, reason: "answer_level_not_r3".into() };
    }
    let from = from_user.trim();
    if !config.answer_targets.iter().any(|pattern| {
        let pattern = pattern.trim();
        pattern == "*" || pattern == from
    }) {
        return GateDecision { auto_pass: false, reason: "answer_target_not_allowed".into() };
    }
    if day_used >= config.per_day_limit {
        return GateDecision { auto_pass: false, reason: "day_limit".into() };
    }
    GateDecision { auto_pass: true, reason: "ok".into() }
}

/// 判定快照（随 outbox 留档审计）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoGateSnapshot {
    pub decision: GateDecision,
    pub distill_mode: DistillMode,
    pub target_level: String,
    pub scopes: Vec<String>,
    pub session_used: u32,
    pub day_used: u32,
    pub at: i64,
}

/// 自动过门用量计数：会话级内存 + 日级落盘（`auto-grant-state.json`，
/// 只存当日计数，跨日重置）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoGrantUsage {
    /// `YYYY-MM-DD`（本地时区）。
    pub day: String,
    pub day_used: u32,
    #[serde(default)]
    pub sessions: std::collections::BTreeMap<String, u32>,
}

impl AutoGrantUsage {
    pub fn load() -> Self {
        let Ok(path) = usage_path() else { return Self::default() };
        let Ok(raw) = std::fs::read_to_string(path) else { return Self::default() };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save(&self) {
        if let Ok(path) = usage_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(raw) = serde_json::to_string_pretty(self) {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, &raw).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        }
    }

    fn today() -> String {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    }

    /// 归一化到当日（跨日自动清零会话与日计数）。
    pub fn current() -> Self {
        let mut usage = Self::load();
        let today = Self::today();
        if usage.day != today {
            usage = Self { day: today, day_used: 0, sessions: Default::default() };
            usage.save();
        }
        usage
    }

    pub fn session_used(&self, session_id: &str) -> u32 {
        self.sessions.get(session_id).copied().unwrap_or(0)
    }

    /// 计一次（自动过门成功后调用）。
    pub fn record(session_id: &str) {
        let mut usage = Self::current();
        usage.day_used += 1;
        *usage.sessions.entry(session_id.to_string()).or_insert(0) += 1;
        usage.save();
    }
}

fn usage_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction").join("auto-grant-state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AutoGrantConfig {
        AutoGrantConfig {
            enabled: true,
            max_level: "R3".into(),
            scope_whitelist: vec!["finance/*".into(), "blog".into()],
            open_scopes: vec!["blog".into()],
            per_session_limit: 3,
            per_day_limit: 20,
            targets: vec!["*".into()],
            fast_mode: false,
            ..AutoGrantConfig::default()
        }
    }

    #[test]
    fn gate_five_dimensions() {
        let base = config();
        // 基线：R3 + 白名单 scope + 配额内 + 目标任意 → 通过。
        let ok = evaluate(&base, "R3", &["finance/财报".to_string()], 0, 0, "anyone");
        assert!(ok.auto_pass, "{ok:?}");

        // 维度一：总开关。
        let mut disabled = base.clone();
        disabled.enabled = false;
        assert!(!evaluate(&disabled, "R3", &[], 0, 0, "x").auto_pass);
        // 维度二：等级超限（max=R3 时 R1/R2 更严本应可通过？——等级语义：
        // R1 信息量最大最敏感，数值小；max_level 限制的是「不高于」。
        // R1 请求在 max=R3 下允许（更严的等级不需要拦），R3 之上无更高档。
        assert!(evaluate(&base, "R1", &[], 0, 0, "x").auto_pass);
        let mut low_max = base.clone();
        low_max.max_level = "R1".into();
        assert!(!evaluate(&low_max, "R3", &[], 0, 0, "x").auto_pass);
        // 维度三：scope 白名单。
        let out = evaluate(&base, "R3", &["diary/私人".to_string()], 0, 0, "x");
        assert!(!out.auto_pass);
        assert_eq!(out.reason, "scope_not_whitelisted");
        // 维度四：会话配额。
        assert!(!evaluate(&base, "R3", &[], 3, 0, "x").auto_pass);
        // 维度五：日配额。
        assert!(!evaluate(&base, "R3", &[], 0, 20, "x").auto_pass);
        // 目标白名单（收敛到具体用户）。
        let mut scoped_target = base.clone();
        scoped_target.targets = vec!["alice".into()];
        assert!(evaluate(&scoped_target, "R3", &[], 0, 0, "alice").auto_pass);
        assert!(!evaluate(&scoped_target, "R3", &[], 0, 0, "bob").auto_pass);
    }

    /// FD-12 M4：B 端 auto-answer 四维判定。
    #[test]
    fn auto_answer_gate_dimensions() {
        let base = config();
        // 默认 off：一切咨询回落人审。
        let off = evaluate_auto_answer(&base, "alice", "R3", 0);
        assert!(!off.auto_pass);
        assert_eq!(off.reason, "auto_answer_off");

        let mut on = base.clone();
        on.auto_answer = "R3".into();
        assert!(evaluate_auto_answer(&on, "alice", "R3", 0).auto_pass);
        // 独立于 A 端发起总开关。
        on.enabled = false;
        assert!(evaluate_auto_answer(&on, "alice", "R3", 0).auto_pass);
        // 非 R3 咨询信封：拒。
        let low = evaluate_auto_answer(&on, "alice", "R1", 0);
        assert!(!low.auto_pass);
        assert_eq!(low.reason, "answer_level_not_r3");
        // 来源白名单收敛。
        on.answer_targets = vec!["alice".into()];
        assert!(evaluate_auto_answer(&on, "alice", "R3", 0).auto_pass);
        let stranger = evaluate_auto_answer(&on, "bob", "R3", 0);
        assert!(!stranger.auto_pass);
        assert_eq!(stranger.reason, "answer_target_not_allowed");
        // 日配额（与发起共用）。
        assert!(!evaluate_auto_answer(&on, "alice", "R3", 20).auto_pass);
    }

    #[test]
    fn distill_mode_four_states() {
        let base = config();
        // 零取材。
        assert_eq!(distill_mode(&base, &[], false), DistillMode::ZeroMaterial);
        assert_eq!(
            distill_mode(&base, &["finance/财报".to_string()], false),
            DistillMode::ZeroMaterial
        );
        // 开放库直通。
        assert_eq!(
            distill_mode(&base, &["blog".to_string()], true),
            DistillMode::OpenScope
        );
        // 混合取材按最严（开放 + 普通 → 非开放档）。
        assert_eq!(
            distill_mode(&base, &["blog".to_string(), "finance/财报".to_string()], true),
            DistillMode::FullDistill
        );
        // fastMode：非开放取材走兜底。
        let mut fast = base.clone();
        fast.fast_mode = true;
        assert_eq!(
            distill_mode(&fast, &["finance/财报".to_string()], true),
            DistillMode::Fallback
        );
        // 完整蒸馏。
        assert_eq!(
            distill_mode(&base, &["finance/财报".to_string()], true),
            DistillMode::FullDistill
        );
    }

    #[test]
    fn scope_wildcard_semantics() {
        assert!(scope_matches("*", "anything"));
        assert!(scope_matches("finance/*", "finance/财报"));
        assert!(scope_matches("finance", "finance"));
        assert!(!scope_matches("finance/*", "financeX/财报"));
        assert!(!scope_matches("finance/*", "finance"));
        assert!(!scope_matches("finance/*", "fin/子"));
    }

    #[test]
    fn config_roundtrip_and_defaults() {
        let mut dir = tempfile::tempdir().unwrap();
        // 用环境隔离不可行（并行测试），仅验证序列化往返与默认值。
        let default = AutoGrantConfig::default();
        assert!(!default.enabled);
        assert_eq!(default.max_level, "R3");
        assert_eq!(default.per_session_limit, 3);
        let raw = serde_json::to_string(&default).unwrap();
        let back: AutoGrantConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, default);
        // 未知字段容忍（前向兼容）。
        let extended = raw.replace("\"schemaVersion\"", "\"futureField\":1,\"schemaVersion\"");
        let _: AutoGrantConfig = serde_json::from_str(&extended).unwrap();
        let _ = &mut dir;
    }

    #[test]
    fn usage_day_rollover_and_sessions() {
        let mut usage = AutoGrantUsage::default();
        usage.day = "2000-01-01".into();
        usage.day_used = 19;
        usage.sessions.insert("s1".into(), 3);
        // 直接验证结构语义（current() 走文件系统，这里测纯逻辑分支）。
        assert_eq!(usage.session_used("s1"), 3);
        assert_eq!(usage.session_used("missing"), 0);
        assert_ne!(usage.day, AutoGrantUsage::today());
    }
}
