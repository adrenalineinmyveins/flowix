use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// The declaration-only plugin manifest shared by Desktop and CLI.
///
/// The fields are intentionally data-only. A manifest can describe a tool,
/// its output and requested host capabilities, but it cannot cause arbitrary
/// code to execute in the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub ui: PluginUi,
    pub input: PluginInput,
    #[serde(default)]
    pub agent: Option<PluginAgent>,
    #[serde(default)]
    pub tool: Option<PluginTool>,
    #[serde(default)]
    pub discovery: PluginDiscovery,
    #[serde(default)]
    pub execution: PluginExecution,
    #[serde(default)]
    pub engines: PluginEngines,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub integrity: Option<PluginIntegrity>,
    pub output: PluginOutput,
    /// schema v3：P2 知识绑定（tag-path 数组，运行时按 scope 检索注入）。
    #[serde(default)]
    pub knowledge_scope: Vec<String>,
    /// schema v3：P4 工具授权（read/write/confirm 三档 + allowlist）。
    #[serde(default)]
    pub tool_grants: Vec<PluginToolGrant>,
    /// schema v3：F6 联动——网络咨询时该技能允许的最高披露等级。
    #[serde(default)]
    pub min_redaction_level: Option<String>,
}

/// P4 工具授权声明。`tool` 白名单：flowix.memo / flowix.notebook / shell /
/// artifact / flowix.consult（FD-12 远程咨询，read=预授权内直发）；
/// `access` ∈ read | write | confirm（confirm 走 DSH approval）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolGrant {
    pub tool: String,
    pub access: String,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTool {
    pub command: String,
    pub input: String,
    pub content_type: String,
    pub instructions: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PluginParser {
    MindmapMarkdown,
    Markdown,
    Json,
    Html,
    Text,
}

impl PluginParser {
    pub fn parse(raw: &str, format: &str) -> Result<Self, String> {
        let normalized = if raw.trim().is_empty() {
            match format {
                "markdown" => "mindmap-markdown",
                "json" => "json",
                "html" => "html",
                "text" => "text",
                _ => return Err(format!("unsupported plugin output format: {format}")),
            }
        } else {
            raw.trim()
        };
        match normalized {
            "mindmap-markdown" => Ok(Self::MindmapMarkdown),
            "markdown" => Ok(Self::Markdown),
            "json" => Ok(Self::Json),
            "html" => Ok(Self::Html),
            "text" => Ok(Self::Text),
            _ => Err(format!("unsupported plugin output parser: {normalized}")),
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::MindmapMarkdown => "mindmap-markdown",
            Self::Markdown => "markdown",
            Self::Json => "json",
            Self::Html => "html",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PluginRuntime {
    Codex,
    Claude,
    Hermes,
    OpenCode,
}

impl PluginRuntime {
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, String> {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let runtime = match raw.to_ascii_lowercase().as_str() {
            "codex" => Self::Codex,
            "claude" => Self::Claude,
            "hermes" => Self::Hermes,
            "opencode" => Self::OpenCode,
            _ => return Err(format!("unsupported plugin runtime: {raw}")),
        };
        Ok(Some(runtime))
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Hermes => "hermes",
            Self::OpenCode => "opencode",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginDefinition {
    pub parser: PluginParser,
    pub runtime: Option<PluginRuntime>,
    pub output_directory: PathBuf,
    pub extension: String,
    pub note_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginUi {
    pub placement: String,
    pub order: i32,
    pub icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInput {
    #[serde(default)]
    pub fields: Vec<PluginField>,
    #[serde(default)]
    pub prompt: Option<PluginField>,
    #[serde(default)]
    pub agent_type: Option<PluginField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginField {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub options: Vec<PluginOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAgent {
    pub skill: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiscovery {
    #[serde(default)]
    pub note_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginExecution {
    #[serde(default)]
    pub runtime: Option<String>,
    /// schema v3：单次运行 token 预算上限（超限取消并留痕）。
    #[serde(default)]
    pub budget_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginEngines {
    #[serde(default)]
    pub flowix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIntegrity {
    pub algorithm: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOutput {
    pub format: String,
    pub directory: String,
    pub extension: String,
    pub renderer: String,
    #[serde(default)]
    pub parser: String,
}

/// schema v3 专属校验：知识绑定 / 工具授权 / 披露等级 / 预算。
///
/// v1/v2 清单的新字段经 serde default 为空，本函数不运行（U12 兼容）。
fn validate_manifest_v3(manifest: &PluginManifest) -> Result<(), String> {
    for tag in &manifest.knowledge_scope {
        if !looks_like_tag_path(tag) {
            return Err(format!(
                "plugin knowledgeScope entry is not a tag path: {tag}"
            ));
        }
    }
    let mut granted = HashSet::new();
    for grant in &manifest.tool_grants {
        if !matches!(
            grant.tool.as_str(),
            "flowix.memo" | "flowix.notebook" | "shell" | "artifact"
        ) {
            return Err(format!("unsupported plugin tool grant: {}", grant.tool));
        }
        if !matches!(grant.access.as_str(), "read" | "write" | "confirm") {
            return Err(format!(
                "unsupported tool grant access '{}': {}",
                grant.access, grant.tool
            ));
        }
        if !granted.insert(grant.tool.as_str()) {
            return Err(format!("duplicate tool grant: {}", grant.tool));
        }
        if grant.tool == "shell" && grant.access == "confirm" && grant.allowlist.is_empty() {
            return Err("shell confirm grant requires a non-empty allowlist".into());
        }
        for entry in &grant.allowlist {
            if entry.trim().is_empty() {
                return Err(format!("tool grant allowlist has an empty entry: {}", grant.tool));
            }
        }
    }
    if let Some(level) = &manifest.min_redaction_level {
        if !matches!(level.as_str(), "R1" | "R2" | "R3") {
            return Err(format!("unsupported minRedactionLevel: {level}"));
        }
    }
    if let Some(budget) = manifest.execution.budget_tokens {
        if budget == 0 {
            return Err("plugin execution budgetTokens must be positive".into());
        }
    }
    Ok(())
}

/// 轻量 tag-path 形状检查（完整归一化在 desktop 侧走 flowix-core 的
/// `normalize_tag_path`，此 crate 不依赖 core）。
fn looks_like_tag_path(raw: &str) -> bool {
    !raw.trim().is_empty()
        && !raw.starts_with('/')
        && !raw.ends_with('/')
        && !raw.contains("//")
        && !raw.contains(char::is_whitespace)
}

pub fn valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().enumerate().all(|(index, ch)| {
            (ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
                && (index > 0 || ch.is_ascii_lowercase())
        })
}

pub fn is_relative_plugin_path(raw: &str) -> bool {
    let path = Path::new(raw);
    !raw.trim().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

pub fn validate_manifest(manifest: &PluginManifest) -> Result<PluginDefinition, String> {
    if !matches!(manifest.schema_version, 1 | 2 | 3) {
        return Err(format!(
            "unsupported plugin schema version: {}",
            manifest.schema_version
        ));
    }
    if manifest.schema_version >= 3 {
        validate_manifest_v3(manifest)?;
    }
    if !valid_plugin_id(&manifest.id) {
        return Err("plugin id must use lowercase letters, numbers, '-' or '_'".into());
    }
    if manifest.name.trim().is_empty() || manifest.version.trim().is_empty() {
        return Err("plugin name and version are required".into());
    }
    if manifest.schema_version >= 2 {
        semver::Version::parse(&manifest.version)
            .map_err(|error| format!("plugin version must use semantic versioning: {error}"))?;
    }
    if let Some(requirement) = manifest
        .engines
        .flowix
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let requirement = semver::VersionReq::parse(requirement)
            .map_err(|error| format!("invalid Flowix engine requirement: {error}"))?;
        let host = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("invalid Flowix host version: {error}"))?;
        if !requirement.matches(&host) {
            return Err(format!(
                "plugin requires Flowix {requirement}, current version is {host}"
            ));
        }
    }
    let mut permissions = HashSet::new();
    for permission in &manifest.permissions {
        if !matches!(
            permission.as_str(),
            "agent.invoke" | "notebook.read" | "artifact.write"
        ) {
            return Err(format!("unsupported plugin permission: {permission}"));
        }
        if !permissions.insert(permission) {
            return Err(format!("duplicate plugin permission: {permission}"));
        }
    }

    let note_type = manifest
        .discovery
        .note_type
        .as_deref()
        .unwrap_or(&manifest.id)
        .trim();
    if !valid_plugin_id(note_type) {
        return Err("plugin discovery noteType is invalid".into());
    }

    match manifest.kind.as_str() {
        "agent-markdown" => {
            let agent = manifest
                .agent
                .as_ref()
                .ok_or_else(|| "agent-markdown plugin requires an agent skill".to_string())?;
            if !is_relative_plugin_path(&agent.skill) {
                return Err("plugin manifest contains an invalid skill".into());
            }
            if let Some(integrity) = &manifest.integrity {
                if !integrity.files.contains_key(&agent.skill) {
                    return Err("plugin integrity must include the skill file".into());
                }
            }
        }
        "artifact-tool" if manifest.schema_version >= 2 => {
            let tool = manifest
                .tool
                .as_ref()
                .ok_or_else(|| "artifact-tool plugin requires a tool declaration".to_string())?;
            if tool.input != "stdin"
                || tool.command.trim().is_empty()
                || tool.content_type.trim().is_empty()
                || !is_relative_plugin_path(&tool.instructions)
            {
                return Err("plugin tool declaration is invalid".into());
            }
            if let Some(integrity) = &manifest.integrity {
                if !integrity.files.contains_key(&tool.instructions) {
                    return Err("plugin integrity must include the instruction file".into());
                }
            }
        }
        other => return Err(format!("unsupported plugin kind: {other}")),
    }

    if manifest.ui.placement != "sidebar" {
        return Err(format!(
            "unsupported plugin UI placement: {}",
            manifest.ui.placement
        ));
    }
    if !is_relative_plugin_path(&manifest.output.directory) {
        return Err("plugin manifest contains an invalid output directory".into());
    }

    let mut field_ids = HashSet::new();
    let fields = if manifest.input.fields.is_empty() {
        manifest
            .input
            .prompt
            .iter()
            .map(|field| (field, "prompt"))
            .chain(
                manifest
                    .input
                    .agent_type
                    .iter()
                    .map(|field| (field, "agentType")),
            )
            .collect::<Vec<_>>()
    } else {
        manifest
            .input
            .fields
            .iter()
            .map(|field| (field, field.id.as_str()))
            .collect::<Vec<_>>()
    };
    for (field, fallback_id) in fields {
        let field_id = if field.id.trim().is_empty() {
            fallback_id
        } else {
            field.id.as_str()
        };
        if !field_ids.insert(field_id) {
            return Err("plugin input field ids must be unique and non-empty".into());
        }
        if !matches!(
            field.field_type.as_str(),
            "text" | "input" | "textarea" | "select" | "agent-select" | "number" | "checkbox"
        ) {
            return Err(format!(
                "unsupported plugin input field type: {}",
                field.field_type
            ));
        }
        if matches!(field.field_type.as_str(), "select" | "agent-select") {
            let mut option_values = HashSet::new();
            if field.options.is_empty()
                || field.options.iter().any(|option| {
                    option.value.trim().is_empty()
                        || option.label.trim().is_empty()
                        || !option_values.insert(option.value.as_str())
                })
            {
                return Err(format!("plugin field '{field_id}' has invalid options"));
            }
        }
    }

    let parser = PluginParser::parse(&manifest.output.parser, &manifest.output.format)?;
    let expected_format = match parser {
        PluginParser::MindmapMarkdown | PluginParser::Markdown => "markdown",
        PluginParser::Json => "json",
        PluginParser::Html => "html",
        PluginParser::Text => "text",
    };
    if manifest.output.format != expected_format {
        return Err(format!(
            "plugin parser does not match output format: {} vs {}",
            manifest.output.parser, manifest.output.format
        ));
    }
    let extension = manifest.output.extension.trim().trim_start_matches('.');
    if extension.is_empty()
        || extension.len() > 16
        || !extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return Err("plugin manifest contains an invalid output extension".into());
    }
    match (parser, manifest.output.renderer.as_str()) {
        (PluginParser::MindmapMarkdown, "markmap")
        | (PluginParser::Json, "json-viewer")
        | (PluginParser::Html, "html" | "webpage")
        | (PluginParser::Markdown, "markdown" | "text")
        | (PluginParser::Text, "text") => {}
        (_, renderer) => return Err(format!("unsupported plugin output renderer: {renderer}")),
    }

    if let Some(integrity) = &manifest.integrity {
        if integrity.algorithm != "sha256" || integrity.files.is_empty() {
            return Err("plugin integrity declaration is invalid".into());
        }
        for (path, hash) in &integrity.files {
            if path == "plugin.json"
                || !is_relative_plugin_path(path)
                || hash.len() != 64
                || !hash.chars().all(|ch| ch.is_ascii_hexdigit())
            {
                return Err(format!("plugin integrity entry is invalid: {path}"));
            }
        }
    }

    Ok(PluginDefinition {
        parser,
        runtime: PluginRuntime::parse(manifest.execution.runtime.as_deref())?,
        output_directory: PathBuf::from(".flowix").join("plugin").join(&manifest.id),
        extension: format!(".{extension}"),
        note_type: note_type.into(),
    })
}
