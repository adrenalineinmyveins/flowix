//! 个人 Agent 身份卡数据模型。
//!
//! Profile 是一篇 Markdown 笔记（`.agents/<agent_id>/profile.md`），其
//! frontmatter 携带 `flowix_` 前缀的系统字段，正文是用户可编辑的人设。
//! 本模块只负责 frontmatter 的解析、校验与序列化，文件 IO 由调用方负责。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::memo_file::{
    extract_body_content, extract_frontmatter_properties, normalize_tag_path,
};

const PROFILE_KIND: &str = "agent-profile";
const MAX_BIO_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionLevel {
    R1,
    R2,
    R3,
}

impl RedactionLevel {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "R1" => Some(Self::R1),
            "R2" => Some(Self::R2),
            "R3" => Some(Self::R3),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::R1 => "R1",
            Self::R2 => "R2",
            Self::R3 => "R3",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("frontmatter 缺少 flowix_kind=agent-profile")]
    NotProfile,
    #[error("flowix_agent_id 缺失")]
    MissingAgentId,
    #[error("flowix_display_name 缺失")]
    MissingDisplayName,
    #[error("flowix_bio 超过 {0} 字符")]
    BioTooLong(usize),
    #[error("flowix_expertise 第 {index} 项标签路径非法: {value}")]
    InvalidExpertise { index: usize, value: String },
    #[error("flowix_redaction_level 非法: {0}")]
    InvalidRedactionLevel(String),
}

/// 身份卡 frontmatter 字段集合。
///
/// 所有字段以 `flowix_` 为前缀；正文（人设）不进此结构，由调用方保留。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    #[serde(rename = "flowix_agent_id")]
    pub agent_id: String,
    #[serde(rename = "flowix_display_name")]
    pub display_name: String,
    #[serde(rename = "flowix_bio", default)]
    pub bio: String,
    #[serde(rename = "flowix_expertise", default)]
    pub expertise: Vec<String>,
    #[serde(rename = "flowix_agent_type", default)]
    pub agent_type: String,
    #[serde(rename = "flowix_model", default)]
    pub model: Option<String>,
    #[serde(rename = "flowix_consultable", default = "default_true")]
    pub consultable: bool,
    #[serde(rename = "flowix_redaction_level", default)]
    pub redaction_level: Option<String>,
    #[serde(rename = "flowix_disclosure_scope", default)]
    pub disclosure_scope: Vec<String>,
    #[serde(rename = "flowix_created_at", default)]
    pub created_at: i64,
    #[serde(rename = "flowix_updated_at", default)]
    pub updated_at: i64,
}

fn default_true() -> bool {
    true
}

impl AgentProfile {
    /// 校验字段合法性。expertise 与 disclosure_scope 逐项过 normalize_tag_path。
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.display_name.trim().is_empty() {
            return Err(ProfileError::MissingDisplayName);
        }
        if self.bio.chars().count() > MAX_BIO_CHARS {
            return Err(ProfileError::BioTooLong(MAX_BIO_CHARS));
        }
        for (i, tag) in self.expertise.iter().enumerate() {
            if normalize_tag_path(tag).is_none() {
                return Err(ProfileError::InvalidExpertise {
                    index: i,
                    value: tag.clone(),
                });
            }
        }
        if let Some(level) = &self.redaction_level {
            if RedactionLevel::parse(level).is_none() {
                return Err(ProfileError::InvalidRedactionLevel(level.clone()));
            }
        }
        Ok(())
    }

    /// 从 Markdown 正文解析身份卡。返回 (profile, body)。
    pub fn parse(content: &str) -> Result<(Self, String), ProfileError> {
        let props = extract_frontmatter_properties(content);
        let kind = props
            .get("flowix_kind")
            .and_then(Value::as_str)
            .unwrap_or("");
        if kind != PROFILE_KIND {
            return Err(ProfileError::NotProfile);
        }
        let agent_id = props
            .get("flowix_agent_id")
            .and_then(Value::as_str)
            .ok_or(ProfileError::MissingAgentId)?
            .to_string();
        let mut profile: AgentProfile = serde_json::from_value(props.clone())
            .map_err(|_| ProfileError::MissingDisplayName)?;
        profile.agent_id = agent_id;
        profile.validate()?;
        let body = extract_body_content(content).to_string();
        Ok((profile, body))
    }

    /// 把 profile 字段序列化为完整 Markdown（frontmatter + body）。
    ///
    /// Profile frontmatter 为系统全托管字段，直接确定性构造块，避免
    /// merge_frontmatter 在无前置块时追加的前导换行导致往返不稳定。
    /// body 仅去掉前导空白行，其余字节保留。
    pub fn serialize(&self, body: &str) -> String {
        let mut lines: Vec<String> = vec!["---".into()];
        lines.push(format!("flowix_kind: {PROFILE_KIND}"));
        lines.push(format!("flowix_agent_id: {}", self.agent_id));
        lines.push(format!(
            "flowix_display_name: {}",
            yaml_quote(&self.display_name)
        ));
        lines.push(format!("flowix_bio: {}", yaml_quote(&self.bio)));
        lines.push(format!(
            "flowix_expertise: {}",
            yaml_string_array(&self.expertise)
        ));
        lines.push(format!(
            "flowix_agent_type: {}",
            yaml_quote(&self.agent_type)
        ));
        if let Some(model) = &self.model {
            lines.push(format!("flowix_model: {}", yaml_quote(model)));
        }
        lines.push(format!(
            "flowix_consultable: {}",
            if self.consultable { "true" } else { "false" }
        ));
        if let Some(level) = &self.redaction_level {
            lines.push(format!("flowix_redaction_level: {level}"));
        }
        lines.push(format!(
            "flowix_disclosure_scope: {}",
            yaml_string_array(&self.disclosure_scope)
        ));
        lines.push(format!("flowix_created_at: {}", self.created_at));
        lines.push(format!("flowix_updated_at: {}", self.updated_at));
        lines.push("---".into());
        let body = body.trim_start_matches(['\n', '\r']);
        format!("{}\n{}", lines.join("\n"), body)
    }
}

/// 生成 agent_id：agt_ 前缀 + 8 字节 hex。
pub fn generate_agent_id() -> String {
    use nanoid::nanoid;
    // 8 字节 hex 用 nanoid 字母表的等价长度：16 hex 字符
    let hex: String = nanoid!(16, &[
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ]);
    format!("agt_{hex}")
}

/// Agent 本地运行状态（`<nb>/.flowix/agents/<id>/state.json`）的投影。
///
/// 仅本地：不进云同步、不进 FTS。写入方为桌面运行时（agent run / 咨询
/// 记账），读取方为卡片 UI 与 F2 喂养统计。字段口径见
/// AGENT-PROFILE-MODEL.md §4。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentState {
    pub version: u32,
    pub agent_id: String,
    pub last_run_at: Option<i64>,
    pub last_consulted_at: Option<i64>,
    pub runs: u64,
    pub consultations_received: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// 最近 20 个 threadId，FIFO。
    #[serde(default)]
    pub recent_thread_ids: Vec<String>,
    /// 设备级偏好（不跨设备）。
    #[serde(default)]
    pub local_preferences: serde_json::Value,
}

impl AgentState {
    pub const MAX_RECENT_THREADS: usize = 20;

    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            version: 1,
            agent_id: agent_id.into(),
            last_run_at: None,
            last_consulted_at: None,
            runs: 0,
            consultations_received: 0,
            tokens_in: 0,
            tokens_out: 0,
            recent_thread_ids: Vec::new(),
            local_preferences: serde_json::Value::Null,
        }
    }

    /// 记一次 agent run：runs +1、lastRunAt、threadId 进 FIFO（去重置顶）。
    pub fn record_run(&mut self, thread_id: &str, at_ms: i64) {
        self.runs = self.runs.saturating_add(1);
        self.last_run_at = Some(at_ms);
        self.recent_thread_ids.retain(|id| id != thread_id);
        self.recent_thread_ids.push(thread_id.to_string());
        while self.recent_thread_ids.len() > Self::MAX_RECENT_THREADS {
            self.recent_thread_ids.remove(0);
        }
    }

    /// 记 token 用量（口径同 flowix/session/usage）。
    pub fn record_tokens(&mut self, tokens_in: u64, tokens_out: u64) {
        self.tokens_in = self.tokens_in.saturating_add(tokens_in);
        self.tokens_out = self.tokens_out.saturating_add(tokens_out);
    }
}

fn yaml_quote(s: &str) -> String {
    // 双引号转义，适合 merge_frontmatter 单行替换
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn yaml_string_array(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| yaml_quote(s)).collect();
    format!("[{}]", inner.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> i64 {
        1_700_000_000_000
    }

    #[test]
    fn parses_valid_profile_and_preserves_body() {
        let content = "---\nflowix_kind: agent-profile\nflowix_agent_id: agt_test1234\nflowix_display_name: \"财报专家\"\nflowix_expertise: [\"finance/财报\"]\nflowix_consultable: true\nflowix_created_at: 100\nflowix_updated_at: 200\n---\n# 人设\n\n专注财报分析。\n";
        let (profile, body) = AgentProfile::parse(content).unwrap();
        assert_eq!(profile.agent_id, "agt_test1234");
        assert_eq!(profile.display_name, "财报专家");
        assert_eq!(profile.expertise, vec!["finance/财报".to_string()]);
        assert!(profile.consultable);
        assert!(body.contains("专注财报分析"));
    }

    #[test]
    fn rejects_non_profile_kind() {
        let content = "---\nflowix_kind: note\nflowix_agent_id: agt_x\nflowix_display_name: x\n---\nbody\n";
        assert!(matches!(
            AgentProfile::parse(content),
            Err(ProfileError::NotProfile)
        ));
    }

    #[test]
    fn rejects_invalid_expertise_tag() {
        let mut profile = base_profile();
        profile.expertise = vec!["空格 标签".to_string()];
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::InvalidExpertise { .. })
        ));
    }

    #[test]
    fn rejects_bio_over_500_chars() {
        let mut profile = base_profile();
        profile.bio = "a".repeat(501);
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::BioTooLong(_))
        ));
    }

    #[test]
    fn serialize_round_trip_preserves_body_and_fields() {
        let profile = base_profile();
        let body = "# 人设\n\n正文内容。\n";
        let serialized = profile.serialize(body);
        let (parsed, parsed_body) = AgentProfile::parse(&serialized).unwrap();
        assert_eq!(parsed, profile);
        assert_eq!(parsed_body, body);
        // 二次往返字节稳定。
        let reserialized = parsed.serialize(&parsed_body);
        assert_eq!(reserialized, serialized);
    }

    #[test]
    fn generate_agent_id_format() {
        let id = generate_agent_id();
        assert!(id.starts_with("agt_"));
        assert_eq!(id.len(), 4 + 16);
    }

    #[test]
    fn agent_state_record_run_fifo_and_dedup() {
        let mut state = AgentState::new("agt_test");
        state.record_run("t1", 100);
        state.record_run("t2", 200);
        assert_eq!(state.recent_thread_ids, vec!["t1", "t2"]);
        assert_eq!(state.runs, 2);
        assert_eq!(state.last_run_at, Some(200));
        // 重复 threadId 去重置顶，不重复计数位置。
        state.record_run("t1", 300);
        assert_eq!(state.recent_thread_ids, vec!["t2", "t1"]);
        assert_eq!(state.runs, 3);
    }

    #[test]
    fn agent_state_recent_threads_capped_at_20() {
        let mut state = AgentState::new("agt_test");
        for i in 0..25 {
            state.record_run(&format!("t{i}"), i as i64);
        }
        assert_eq!(state.recent_thread_ids.len(), AgentState::MAX_RECENT_THREADS);
        assert_eq!(state.recent_thread_ids.first().map(String::as_str), Some("t5"));
        assert_eq!(state.recent_thread_ids.last().map(String::as_str), Some("t24"));
    }

    #[test]
    fn agent_state_json_round_trip() {
        let mut state = AgentState::new("agt_test");
        state.record_run("thr_1", 1000);
        state.record_tokens(120, 30);
        state.local_preferences = serde_json::json!({ "contextWindow": 128000 });
        let encoded = serde_json::to_string(&state).unwrap();
        let decoded: AgentState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, state);
        assert_eq!(decoded.runs, 1);
        assert_eq!(decoded.tokens_in, 120);
        assert_eq!(decoded.tokens_out, 30);
    }

    /// 集成（设计修订验证）：`.agents/` 属于点开头目录，被
    /// FileManagementPolicy 整体排除在 note_index 之外（与
    /// `.agents/agents/agent.md` 子代理配置同等待遇）。因此卡片 UI 的
    /// 数据源是 `agent_profile_list`（直扫目录），profile 不进入笔记
    /// 搜索与默认列表。此测试锁定该边界，防止未来误把 profile 当笔记
    /// 索引（那会污染用户笔记列表）。
    #[test]
    fn profile_note_is_excluded_from_note_index() {
        use crate::memo_file::FileManagementPolicy;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("notebook");
        std::fs::create_dir_all(root.join(".agents").join("agt_projtest")).unwrap();
        let policy = FileManagementPolicy::default();
        let relative = std::path::Path::new(".agents/agt_projtest/profile.md");
        assert!(
            policy.is_index_ignored_at(&root, relative),
            ".agents profile must stay out of the note index"
        );
        // 普通笔记不受影响。
        assert!(!policy.is_index_ignored_at(&root, std::path::Path::new("notes/a.md")));
    }

    fn base_profile() -> AgentProfile {
        AgentProfile {
            agent_id: "agt_test1234".into(),
            display_name: "测试专家".into(),
            bio: "一句话简介".into(),
            expertise: vec!["finance/财报".into()],
            agent_type: "codex".into(),
            model: None,
            consultable: true,
            redaction_level: Some("R2".into()),
            disclosure_scope: vec![],
            created_at: now(),
            updated_at: now(),
        }
    }
}
