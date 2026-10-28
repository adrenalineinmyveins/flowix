//! Model Context Protocol stdio frontend for external Agents.
//!
//! The server intentionally exposes exactly one tool, `memo`. Its input is a
//! restricted Flowix CLI command plus optional stdin content. Commands are parsed into
//! argv and dispatched directly to the typed store layer; no system shell is spawned.

use crate::{cli, errors::CliError, fmt, operation, output, path_store, plugin, store};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};

pub const TOOL_NAME: &str = "memo";
pub const CONSULT_TOOL_NAME: &str = "consult_remote";
pub const CONSULT_TARGETS_TOOL_NAME: &str = "consult_targets";
pub const AGENT_PROFILE_TOOL_NAME: &str = "agent_profile";
const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    LATEST_PROTOCOL_VERSION,
];

pub const TOOL_DESCRIPTION: &str = "Search and manage Flowix Markdown notes, read table and media library collection datasets by collection ID or notebook-relative path, and use declared artifacts. Delete is destructive.";

/// FD-12 M2：远程咨询工具。默认抽象提问（零取材直通，毫秒级出门）；
/// scopeHint 仅在需要引用笔记原文时携带。回执立即返回，回程异步到达。
pub const CONSULT_TOOL_DESCRIPTION: &str = "Consult another user's published Flowix agent over the network (asynchronous). Ask in abstract terms by default: a question written from scratch leaves in milliseconds. Include scopeHint only when the answer must draw on your notes; scopes outside the pre-authorized whitelist fall back to manual review. Returns a delivery receipt (receiptId, status); the answer arrives asynchronously and will be injected into this conversation later. Target accepts a full address (user/agent@relay), or a short form (agent slug, user/agent) which is completed locally; call consult_targets to list reachable agents.";

/// Run the MCP line-delimited JSON-RPC loop until stdin reaches EOF.
pub fn run_mcp<R: BufRead, W: Write>(reader: R, mut writer: W) -> Result<(), CliError> {
    for line in reader.lines() {
        let line = line.map_err(CliError::Io)?;
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                write_json_line(
                    &mut writer,
                    &rpc_error(Value::Null, -32700, format!("parse error: {error}")),
                )?;
                continue;
            }
        };

        // MCP notifications do not receive responses.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        let response = match method {
            "initialize" => rpc_result(id, initialize_result(&params)),
            "ping" => rpc_result(id, json!({})),
            "tools/list" => rpc_result(id, json!({"tools": [tool_definition(), consult_tool_definition(), consult_targets_tool_definition(), agent_profile_tool_definition()]})),
            "tools/call" => match call_tool(&params) {
                Ok(result) => rpc_result(id, result),
                Err(error) => rpc_error(id, -32602, error.to_string()),
            },
            _ => rpc_error(id, -32601, format!("method not found: {method}")),
        };
        write_json_line(&mut writer, &response)?;
    }
    Ok(())
}

fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let protocol_version = requested
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(LATEST_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "flowix-memo",
            "title": "Flowix Memo",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Use the memo tool to search and manage Flowix Markdown memos, read collection datasets with collection.read by collectionId or notebook-relative path, and create declared plugin artifacts such as mind maps."
    })
}

fn tool_definition() -> Value {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../dsh-flowix-memory/memo-tool-schema.json"
    ))
    .expect("memo tool schema must be valid JSON");
    json!({
        "name": TOOL_NAME,
        "title": "Flowix Memo",
        "description": TOOL_DESCRIPTION,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false}
    })
}

fn consult_tool_definition() -> Value {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../dsh-flowix-memory/consult-tool-schema.json"
    ))
    .expect("consult tool schema must be valid JSON");
    json!({
        "name": CONSULT_TOOL_NAME,
        "title": "Flowix Remote Consult",
        "description": CONSULT_TOOL_DESCRIPTION,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
    })
}

fn consult_targets_tool_definition() -> Value {
    json!({
        "name": CONSULT_TARGETS_TOOL_NAME,
        "title": "Flowix Consult Targets",
        "description": "List agents reachable via consult_remote on this device (local registered agents completed to full user/agent@relay addresses). Call this first when you do not know the target address.",
        "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

/// AGENT-PROFILE-REDESIGN P2：对话内 agent 配置工具（直读写
/// `<cwd>/.agents/<id>/profile.md`，与 memo 同权）。写权执法：内容字段
/// 白名单 + memory 非对称闸门（缩可行、扩引导 UI——记忆库即披露授权）。
fn agent_profile_tool_definition() -> Value {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../dsh-flowix-memory/agent-profile-tool-schema.json"
    ))
    .expect("agent profile tool schema must be valid JSON");
    json!({
        "name": AGENT_PROFILE_TOOL_NAME,
        "title": "Flowix Agent Profile",
        "description": "Read and update this user's Flowix agents (name, bio, system prompt, model, tools, skills, metadata, long-term memory). memory binds both what the agent remembers and what it may disclose when consulted; removing memory sources is allowed here, adding them must be done in the app's Agent page.",
        "inputSchema": schema,
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
    })
}

fn call_tool(params: &Value) -> Result<Value, CliError> {
    let object = params
        .as_object()
        .ok_or_else(|| CliError::Usage("tools/call params must be an object".into()))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::Usage("tools/call params.name must be a string".into()))?;
    if name == CONSULT_TOOL_NAME {
        return call_consult_tool(object);
    }
    if name == CONSULT_TARGETS_TOOL_NAME {
        return call_consult_targets_tool();
    }
    if name == AGENT_PROFILE_TOOL_NAME {
        return call_agent_profile_tool(object);
    }
    if name != TOOL_NAME {
        return Err(CliError::Usage(format!("unknown tool: `{name}`")));
    }
    let arguments = object
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Usage("memo arguments must be an object".into()))?;
    validate_argument_keys(arguments)?;
    if arguments.contains_key("action") && arguments.contains_key("command") {
        return Err(CliError::Usage(
            "memo accepts either structured `action` or legacy `command`, not both".into(),
        ));
    }
    let stdin = match arguments.get("stdin") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| CliError::Usage("memo.stdin must be a string".into()))?,
        ),
        None => None,
    };

    let execution = if arguments.contains_key("action") {
        if stdin.is_some() {
            return Err(CliError::Usage(
                "structured actions use `content`; legacy `stdin` is not allowed".into(),
            ));
        }
        operation::execute(parse_structured_operation(arguments)?)
    } else {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| CliError::Usage("memo requires `action` or legacy `command`".into()))?;
        execute_command(command, stdin)
    };

    match execution {
        Ok(data) => Ok(tool_result(data, false)),
        Err(error) => {
            let error_data = json!({
                "ok": false,
                "error": {
                    "code": error_code(&error),
                    "message": error.to_string()
                }
            });
            Ok(tool_result(error_data, true))
        }
    }
}

fn validate_argument_keys(arguments: &Map<String, Value>) -> Result<(), CliError> {
    const KEYS: &[&str] = &[
        "action",
        "notebook",
        "address",
        "query",
        "content",
        "old",
        "limit",
        "offset",
        "dryRun",
        "pluginId",
        "sourceNote",
        "producer",
        "collectionId",
        "path",
        "tag",
        "command",
        "stdin",
    ];
    if let Some(key) = arguments.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(CliError::Usage(format!(
            "memo does not accept argument `{key}`"
        )));
    }
    Ok(())
}

fn parse_structured_operation(
    arguments: &Map<String, Value>,
) -> Result<operation::FlowixOperation, CliError> {
    use operation::FlowixOperation;
    let action = required_string(arguments, "action")?;
    let notebook = optional_string(arguments, "notebook")?;
    let content = || required_string(arguments, "content");
    let address = || required_string(arguments, "address");
    let limit = integer(arguments, "limit", 50, 1, 200)?;
    let offset = integer(arguments, "offset", 0, 0, usize::MAX)?;
    let dry_run = boolean(arguments, "dryRun", false)?;
    match action.as_str() {
        "notebooks" => Ok(FlowixOperation::Notebooks),
        "collection.read" => {
            let notebook = required_string(arguments, "notebook")?;
            Ok(FlowixOperation::CollectionRead {
                notebook,
                collection_id: optional_string(arguments, "collectionId")?,
                path: optional_string(arguments, "path")?,
            })
        }
        "list" => Ok(FlowixOperation::List {
            notebook,
            limit,
            offset,
        }),
        "tags" => Ok(FlowixOperation::Tags { notebook }),
        "show" => Ok(FlowixOperation::Show { address: address()? }),
        "search" => Ok(FlowixOperation::Search {
            query: required_string(arguments, "query")?,
            notebook,
            tag: optional_string(arguments, "tag")?,
            limit,
        }),
        "create" => Ok(FlowixOperation::Create {
            notebook,
            content: content()?,
        }),
        "edit" => Ok(FlowixOperation::Edit {
            address: address()?,
            old: required_string(arguments, "old")?,
            replacement: content()?,
            dry_run,
        }),
        "write" => Ok(FlowixOperation::Write {
            address: address()?,
            content: content()?,
        }),
        "delete" => Ok(FlowixOperation::Delete { address: address()? }),
        "artifact.list" => Ok(FlowixOperation::ArtifactList),
        "artifact.describe" => Ok(FlowixOperation::ArtifactDescribe {
            plugin_id: required_string(arguments, "pluginId")?,
        }),
        "artifact.create" => Ok(FlowixOperation::ArtifactCreate {
            plugin_id: required_string(arguments, "pluginId")?,
            notebook,
            source_note: optional_string(arguments, "sourceNote")?,
            producer: optional_string(arguments, "producer")?.unwrap_or_else(|| "agent-mcp".into()),
            content: content()?,
        }),
        _ => Err(CliError::Usage(format!("unknown memo action: `{action}`"))),
    }
}

fn required_string(arguments: &Map<String, Value>, key: &str) -> Result<String, CliError> {
    optional_string(arguments, key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CliError::Usage(format!("memo.{key} must be a non-empty string")))
}

fn optional_string(arguments: &Map<String, Value>, key: &str) -> Result<Option<String>, CliError> {
    arguments
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| CliError::Usage(format!("memo.{key} must be a string")))
        })
        .transpose()
}

fn integer(
    arguments: &Map<String, Value>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, CliError> {
    let value = match arguments.get(key) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| CliError::Usage(format!("memo.{key} must be an integer")))?
            as usize,
        None => default,
    };
    if value < min || value > max {
        return Err(CliError::Usage(format!(
            "memo.{key} must be between {min} and {max}"
        )));
    }
    Ok(value)
}

fn boolean(arguments: &Map<String, Value>, key: &str, default: bool) -> Result<bool, CliError> {
    arguments
        .get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| CliError::Usage(format!("memo.{key} must be a boolean")))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn execute_command(command: &str, stdin: Option<&str>) -> Result<Value, CliError> {
    reject_shell_syntax(command)?;
    let args = shell_words::split(command)
        .map_err(|error| CliError::Usage(format!("invalid command quoting: {error}")))?;
    if args.is_empty() {
        return Err(CliError::Usage("memo.command cannot be empty".into()));
    }
    if args[0] == "flowix" || args[0] == "flowix-cli" {
        return Err(CliError::Usage(
            "omit the leading `flowix`; pass only the subcommand".into(),
        ));
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h" | "--version" | "-V"))
    {
        return Err(CliError::Usage(
            "help and version commands are not available through memo".into(),
        ));
    }

    let parsed = cli::parse(&args)?.ok_or_else(|| CliError::Usage("missing command".into()))?;
    match parsed {
        cli::Cli::Notebooks { .. } => {
            reject_stdin(stdin)?;
            let (configs, selected_notebook_id) = store::notebooks_list_data()?;
            let counts = store::notebook_note_counts(&configs)?;
            let tag_counts = store::notebook_tag_counts(&configs)?;
            Ok(fmt::notebooks_to_json(
                &configs,
                &counts,
                &tag_counts,
                selected_notebook_id.as_deref(),
            ))
        }
        cli::Cli::List { notebook, .. } => {
            reject_stdin(stdin)?;
            let notebook = store::resolve_notebook_key(notebook.as_deref())?;
            Ok(path_store::list(&notebook)?["notes"].clone())
        }
        cli::Cli::Tags { notebook, .. } => {
            reject_stdin(stdin)?;
            store::notebook_tags(notebook.as_deref())
        }
        cli::Cli::Show { id, .. } => {
            reject_stdin(stdin)?;
            path_store::show(&id)
        }
        cli::Cli::Create { notebook, file, .. } => {
            if file.is_some() {
                return Err(CliError::Usage(
                    "MCP cannot read client-local --file paths; use structured `content`".into(),
                ));
            }
            let body = require_stdin(stdin, "create")?;
            let notebook = store::resolve_notebook_key(notebook.as_deref())?;
            path_store::create(&notebook, body)
        }
        cli::Cli::Delete { id, .. } => {
            reject_stdin(stdin)?;
            path_store::delete(&id)
        }
        cli::Cli::CollectionRead { notebook, collection_id, path, .. } => {
            reject_stdin(stdin)?;
            operation::execute(operation::FlowixOperation::CollectionRead {
                notebook, collection_id, path,
            })
        }
        cli::Cli::Search {
            query,
            notebook,
            tag,
            limit,
            ..
        } => {
            reject_stdin(stdin)?;
            path_store::search(&query, notebook.as_deref(), tag.as_deref(), limit)
        }
        cli::Cli::Edit {
            id,
            old,
            new,
            new_from_stdin,
            new_file,
            dry_run,
            ..
        } => {
            if new_file.is_some() {
                return Err(CliError::Usage(
                    "MCP cannot read client-local --new-file paths; use structured `content`"
                        .into(),
                ));
            }
            let old = old.ok_or_else(|| CliError::Usage("edit requires --old <text>".into()))?;
            let new = if new_from_stdin {
                require_stdin(stdin, "edit --new-stdin")?.to_string()
            } else {
                reject_stdin(stdin)?;
                new.ok_or_else(|| {
                    CliError::Usage("edit requires --new <text> or --new-stdin".into())
                })?
            };
            path_store::edit(&id, &old, &new, dry_run)
        }
        cli::Cli::Write { id, file, .. } => {
            if file.is_some() {
                return Err(CliError::Usage(
                    "MCP cannot read client-local --file paths; use structured `content`".into(),
                ));
            }
            let body = require_stdin(stdin, "write")?;
            path_store::write(&id, body)
        }
        cli::Cli::PluginList { .. } => {
            reject_stdin(stdin)?;
            output::to_json_value(&plugin::list_data())
        }
        cli::Cli::PluginDescribe { plugin_id, .. } => {
            reject_stdin(stdin)?;
            output::to_json_value(&plugin::describe_data(&plugin_id)?)
        }
        cli::Cli::PluginCreate {
            plugin_id,
            notebook,
            source_note,
            producer,
            ..
        } => {
            let content = require_stdin(stdin, "plugin create")?;
            let notebook = store::resolve_notebook_key(notebook.as_deref())?;
            output::to_json_value(&plugin::create_data(
                &plugin_id,
                &notebook,
                source_note.as_deref(),
                &producer,
                content,
            )?)
        }
        cli::Cli::Version | cli::Cli::Completion { .. } | cli::Cli::Mcp => Err(CliError::Usage(
            "command is not available through memo".into(),
        )),
    }
}

fn reject_shell_syntax(command: &str) -> Result<(), CliError> {
    const FORBIDDEN: &[&str] = &["|", ";", "&&", ">", "<", "`", "$(", "${"];
    if let Some(operator) = FORBIDDEN
        .iter()
        .find(|operator| command.contains(**operator))
    {
        return Err(CliError::Usage(format!(
            "shell syntax `{operator}` is not allowed"
        )));
    }
    Ok(())
}

fn require_stdin<'a>(stdin: Option<&'a str>, command: &str) -> Result<&'a str, CliError> {
    let value = stdin.ok_or_else(|| CliError::Usage(format!("{command} requires `stdin`")))?;
    if value.trim().is_empty() {
        return Err(CliError::Usage(format!(
            "{command} requires non-empty `stdin`"
        )));
    }
    Ok(value)
}

fn reject_stdin(stdin: Option<&str>) -> Result<(), CliError> {
    if stdin.is_some() {
        Err(CliError::Usage(
            "stdin is only allowed for create, write, or edit --new-stdin".into(),
        ))
    } else {
        Ok(())
    }
}

// ----------------------------------------------------------------------
// FD-12 M2：consult_remote —— 桥接桌面进程的预授权自动发起链
// ----------------------------------------------------------------------

const BRIDGE_UNAVAILABLE: &str = "BRIDGE_UNAVAILABLE: the Flowix desktop app is not running or its consult bridge is disabled. Start the Flowix desktop app, restart this DSH profile, and retry.";

/// 会话标识：CLI server 进程生命周期 ≈ 一个 DSH 会话上下文，
/// 预授权 perSessionLimit 以此为口径（DSH 重启 = 新进程 = 新计数）。
fn consult_session_id() -> String {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION
        .get_or_init(|| {
            format!(
                "mcp-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_millis()
            )
        })
        .clone()
}

fn consult_tool_result(data: Value, is_error: bool) -> Value {
    let text = if is_error {
        data.get("error")
            .and_then(Value::as_str)
            .unwrap_or("consult_remote failed")
            .to_string()
    } else {
        format!(
            "Consult delivered (receipt {}, status {}, mode {})",
            data.get("receiptId").and_then(Value::as_str).unwrap_or("?"),
            data.get("status").and_then(Value::as_str).unwrap_or("?"),
            data.get("distillMode").and_then(Value::as_str).unwrap_or("?"),
        )
    };
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": data,
        "isError": is_error
    })
}

fn call_consult_tool(params: &Map<String, Value>) -> Result<Value, CliError> {
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Usage("consult_remote arguments must be an object".into()))?;
    let consult_string = |key: &str| -> Result<String, CliError> {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CliError::Usage(format!("consult_remote.{key} must be a non-empty string")))
            .map(str::to_string)
    };
    let target = consult_string("target")?;
    let question = consult_string("question")?;
    let scope_hint = match arguments.get("scopeHint") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| CliError::Usage("consult_remote.scopeHint must be an array of strings".into()))?
            .iter()
            .map(|item| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    CliError::Usage("consult_remote.scopeHint must be an array of strings".into())
                })
            })
            .collect::<Result<Vec<String>, CliError>>()?,
    };
    let cwd = std::env::current_dir()
        .map_err(|error| CliError::Other(format!("resolve cwd: {error}")))?;
    let body = json!({
        "cwd": cwd.to_string_lossy(),
        "question": question,
        "scopePaths": scope_hint,
        // 一期固定 R3（FD-12 §4：maxLevel 默认 R3，等级上限由桌面判定）。
        "targetLevel": "r3",
        "targetAgentId": target,
        "sessionId": consult_session_id(),
    });
    match bridge_roundtrip(&body) {
        Ok(value) => {
            let ok = value.get("ok").and_then(Value::as_bool) == Some(true);
            Ok(consult_tool_result(value, !ok))
        }
        Err(error) => Ok(consult_tool_result(
            json!({"ok": false, "error": error.to_string()}),
            true,
        )),
    }
}

// ----------------------------------------------------------------------
// AGENT-PROFILE-REDESIGN P2：agent_profile —— 对话内配置（直读写文件）
// ----------------------------------------------------------------------

fn agents_root_dir() -> Result<std::path::PathBuf, CliError> {
    let cwd = std::env::current_dir()
        .map_err(|error| CliError::Other(format!("resolve cwd: {error}")))?;
    Ok(cwd.join(".agents"))
}

fn read_agent_profile(agent_id: &str) -> Result<(flowix_core::agent::AgentProfile, String), CliError> {
    let path = agents_root_dir()?.join(agent_id).join("profile.md");
    let content = std::fs::read_to_string(&path)
        .map_err(|_| CliError::Other(format!("AGENT_NOT_FOUND:{agent_id}")))?;
    flowix_core::agent::AgentProfile::parse(&content)
        .map_err(|error| CliError::Other(format!("PROFILE_INVALID:{error}")))
}

fn agent_profile_json(profile: &flowix_core::agent::AgentProfile) -> Value {
    json!({
        "agentId": profile.agent_id,
        "displayName": profile.display_name,
        "bio": profile.bio,
        "expertise": profile.expertise,
        "agentType": profile.agent_type,
        "model": profile.model,
        "consultable": profile.consultable,
        "redactionLevel": profile.redaction_level,
        "system": profile.system,
        "tools": profile.tools,
        "skills": profile.skills,
        "metadata": profile.metadata,
        "memory": profile.memory,
        "createdAt": profile.created_at,
        "updatedAt": profile.updated_at,
    })
}

/// memory 非对称闸门：返回 new 相对 old 的首个新增引用（无新增 = None，
/// 允许——缩小/等价自由）。子树扩大必然引入新字符串，字面差即完备判定。
fn memory_expansion(old: &[String], new: &[String]) -> Option<String> {
    new.iter().find(|item| !old.contains(item)).cloned()
}

fn call_agent_profile_tool(params: &Map<String, Value>) -> Result<Value, CliError> {
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::Usage("agent_profile arguments must be an object".into()))?;
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::Usage("agent_profile.action is required".into()))?;
    match action {
        "list" => {
            let root = agents_root_dir()?;
            let mut agents = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&root) {
                for entry in entries.flatten() {
                    let profile_path = entry.path().join("profile.md");
                    let Ok(content) = std::fs::read_to_string(&profile_path) else { continue };
                    if let Ok((profile, _)) = flowix_core::agent::AgentProfile::parse(&content) {
                        agents.push(json!({
                            "agentId": profile.agent_id,
                            "displayName": profile.display_name,
                            "model": profile.model,
                        }));
                    }
                }
            }
            Ok(json!({
                "content": [{"type": "text", "text": format!("{} agent(s) on this device", agents.len())}],
                "structuredContent": {"agents": agents},
                "isError": false
            }))
        }
        "get" => {
            let agent_id = required_agent_id(arguments)?;
            let (profile, _) = read_agent_profile(&agent_id)?;
            Ok(json!({
                "content": [{"type": "text", "text": format!("Agent {} ({})", profile.display_name, profile.agent_id)}],
                "structuredContent": agent_profile_json(&profile),
                "isError": false
            }))
        }
        "update" => {
            let agent_id = required_agent_id(arguments)?;
            let patch = arguments
                .get("patch")
                .and_then(Value::as_object)
                .ok_or_else(|| CliError::Usage("agent_profile.update requires `patch`".into()))?;
            let (mut profile, body) = read_agent_profile(&agent_id)?;
            for (key, value) in patch {
                match key.as_str() {
                    "displayName" => profile.display_name = string_of(value, "displayName")?,
                    "bio" => profile.bio = string_of(value, "bio")?,
                    "system" => profile.system = string_of(value, "system")?,
                    "model" => {
                        let raw = string_of(value, "model")?;
                        profile.model = if raw.trim().is_empty() { None } else { Some(raw) };
                    }
                    "tools" => profile.tools = string_array_of(value, "tools")?,
                    "skills" => profile.skills = string_array_of(value, "skills")?,
                    "metadata" => {
                        let map = value.as_object().ok_or_else(|| {
                            CliError::Usage("agent_profile.metadata must be an object".into())
                        })?;
                        let mut metadata = std::collections::BTreeMap::new();
                        for (meta_key, meta_value) in map {
                            metadata.insert(
                                meta_key.clone(),
                                meta_value.as_str().unwrap_or_default().to_string(),
                            );
                        }
                        profile.metadata = metadata;
                    }
                    "memory" => {
                        let new_memory = string_array_of(value, "memory")?;
                        if let Some(added) = memory_expansion(&profile.memory, &new_memory) {
                            return Err(CliError::Other(format!(
                                "MEMORY_EXPANSION_REQUIRES_UI:{added}"
                            )));
                        }
                        profile.memory = new_memory;
                    }
                    // D-6：网络纪律字段与结构字段仅 UI 可改。
                    "agentId" | "consultable" | "redactionLevel" | "redaction_level"
                    | "disclosureScope" | "disclosure_scope" | "createdAt" | "updatedAt" => {
                        return Err(CliError::Other(format!("FIELD_NOT_ALLOWED:{key}")));
                    }
                    other => {
                        return Err(CliError::Other(format!("FIELD_NOT_ALLOWED:{other}")));
                    }
                }
            }
            profile.updated_at = chrono::Utc::now().timestamp_millis();
            profile
                .validate()
                .map_err(|error| CliError::Other(format!("PROFILE_INVALID:{error}")))?;
            // 原子写（tmp + rename），与桌面 commands/agent_profile 同契约。
            let path = agents_root_dir()?.join(&agent_id).join("profile.md");
            let temporary = path.with_extension("md.flowix-tmp");
            std::fs::write(&temporary, profile.serialize(&body)).map_err(CliError::Io)?;
            std::fs::rename(&temporary, &path).map_err(CliError::Io)?;
            Ok(json!({
                "content": [{"type": "text", "text": format!("Agent {} updated", profile.display_name)}],
                "structuredContent": agent_profile_json(&profile),
                "isError": false
            }))
        }
        other => Err(CliError::Usage(format!("unknown agent_profile action: `{other}`"))),
    }
}

fn required_agent_id(arguments: &Map<String, Value>) -> Result<String, CliError> {
    arguments
        .get("agentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CliError::Usage("agent_profile.agentId is required".into()))
}

fn string_of(value: &Value, field: &str) -> Result<String, CliError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| CliError::Usage(format!("agent_profile.{field} must be a string")))
}

fn string_array_of(value: &Value, field: &str) -> Result<Vec<String>, CliError> {
    value
        .as_array()
        .ok_or_else(|| CliError::Usage(format!("agent_profile.{field} must be an array of strings")))?
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                CliError::Usage(format!("agent_profile.{field} must be an array of strings"))
            })
        })
        .collect()
}

/// FD-12 M2 补：枚举本机可咨询目标（GET /targets）。无参数；桥接不可用
/// 时与 consult_remote 同口径返回可行动错误。
fn call_consult_targets_tool() -> Result<Value, CliError> {
    let result = bridge_get("/targets");
    let (data, is_error) = match result {
        Ok(value) => {
            let ok = value.get("ok").and_then(Value::as_bool) == Some(true);
            (value, !ok)
        }
        Err(error) => (json!({"ok": false, "error": error.to_string()}), true),
    };
    let text = if is_error {
        data.get("error")
            .and_then(Value::as_str)
            .unwrap_or("consult_targets failed")
            .to_string()
    } else {
        let list = data
            .get("targets")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("address").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        if list.is_empty() {
            "No registered consult targets on this device.".to_string()
        } else {
            format!("Reachable consult targets: {list}")
        }
    };
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": data,
        "isError": is_error
    }))
}

/// 桌面桥接 GET 单请求往返（Connection: close，读至 EOF）。
fn bridge_get(path: &str) -> Result<Value, CliError> {
    use std::io::{Read, Write as _};
    use std::net::TcpStream;
    use std::time::Duration;

    let port: u16 = std::env::var("FLOWIX_CONSULT_BRIDGE_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| CliError::Other(BRIDGE_UNAVAILABLE.to_string()))?;
    let token = std::env::var("FLOWIX_CONSULT_BRIDGE_TOKEN").unwrap_or_default();
    let address: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|_| CliError::Other(BRIDGE_UNAVAILABLE.to_string()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(CliError::Io)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(CliError::Io)?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(CliError::Io)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| CliError::Other("BRIDGE_MALFORMED_RESPONSE".to_string()))?;
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains("200") {
        return Err(CliError::Other(format!(
            "BRIDGE_HTTP_ERROR: {status_line}"
        )));
    }
    serde_json::from_str(body)
        .map_err(|error| CliError::Other(format!("BRIDGE_INVALID_JSON: {error}")))
}

/// 桌面桥接单请求往返（同步 std TCP；Connection: close，读至 EOF）。
/// 读超时 180s：覆盖桌面完整蒸馏的最长路径；直通形态毫秒级返回。
fn bridge_roundtrip(body: &Value) -> Result<Value, CliError> {
    use std::io::{Read, Write as _};
    use std::net::TcpStream;
    use std::time::Duration;

    let port: u16 = std::env::var("FLOWIX_CONSULT_BRIDGE_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| CliError::Other(BRIDGE_UNAVAILABLE.to_string()))?;
    let token = std::env::var("FLOWIX_CONSULT_BRIDGE_TOKEN").unwrap_or_default();
    let payload = serde_json::to_string(body)
        .map_err(|error| CliError::Other(format!("serialize consult body: {error}")))?;
    let address: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|_| CliError::Other(BRIDGE_UNAVAILABLE.to_string()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(180)))
        .map_err(|error| CliError::Io(error))?;
    let request = format!(
        "POST /consult HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(CliError::Io)?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| CliError::Io(error))?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| CliError::Other("BRIDGE_MALFORMED_RESPONSE".to_string()))?;
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains("200") {
        return Err(CliError::Other(format!(
            "BRIDGE_HTTP_ERROR: {status_line}"
        )));
    }
    serde_json::from_str(body)
        .map_err(|error| CliError::Other(format!("BRIDGE_INVALID_JSON: {error}")))
}

fn tool_result(data: Value, is_error: bool) -> Value {
    let text = if is_error {
        data.pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Flowix operation failed")
            .to_string()
    } else {
        let action = data
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("completed");
        let id = data
            .get("address")
            .and_then(Value::as_str)
            .map(|id| format!(" ({id})"))
            .unwrap_or_default();
        format!("Flowix {action}{id}")
    };
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": data,
        "isError": is_error
    })
}

fn error_code(error: &CliError) -> &'static str {
    match error {
        CliError::Usage(_) => "INVALID_COMMAND",
        CliError::NotFound(_) => "NOT_FOUND",
        CliError::Io(_) => "IO_ERROR",
        CliError::Conflict(_) => "CONFLICT",
        CliError::Other(_) => "EXECUTION_ERROR",
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i32, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn write_json_line<W: Write>(writer: &mut W, value: &Value) -> Result<(), CliError> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| CliError::Other(format!("json write: {error}")))?;
    writer.write_all(b"\n").map_err(CliError::Io)?;
    writer.flush().map_err(CliError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(input: &str) -> Vec<Value> {
        let mut output = Vec::new();
        run_mcp(Cursor::new(input.as_bytes()), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn lists_memo_and_consult_tools() {
        let responses = run(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#);
        let tools = responses[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0]["name"], TOOL_NAME);
        assert!(tools[0]["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "create"));
        assert!(tools[0]["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "artifact.create"));
        assert!(tools[0]["inputSchema"]["properties"]["tag"].is_object());
        assert_eq!(
            tools[0]["inputSchema"]["anyOf"],
            json!([{"required":["action"]},{"required":["command"]}])
        );
        let consult = &tools[1];
        assert_eq!(consult["name"], CONSULT_TOOL_NAME);
        assert_eq!(
            consult["inputSchema"]["required"],
            json!(["target", "question"])
        );
        assert!(consult["inputSchema"]["properties"]["scopeHint"].is_object());
    }

    #[test]
    fn consult_requires_target_and_question() {
        let responses = run(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"consult_remote","arguments":{"target":"alice/researcher@https://relay.example"}}}"#,
        );
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("question"));
    }

    #[test]
    fn consult_without_bridge_returns_actionable_error() {
        // 无 FLOWIX_CONSULT_BRIDGE_PORT（测试进程环境）→ isError 工具结果，
        // 非 protocol error：LLM 能读到指引文本。
        let saved = std::env::var("FLOWIX_CONSULT_BRIDGE_PORT").ok();
        std::env::remove_var("FLOWIX_CONSULT_BRIDGE_PORT");
        let responses = run(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"consult_remote","arguments":{"target":"alice/researcher@https://relay.example","question":"abstract question"}}}"#,
        );
        if let Some(saved) = saved {
            std::env::set_var("FLOWIX_CONSULT_BRIDGE_PORT", saved);
        }
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], json!(true));
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("BRIDGE_UNAVAILABLE"));
    }

    #[test]
    fn consult_targets_without_bridge_returns_actionable_error() {
        let saved = std::env::var("FLOWIX_CONSULT_BRIDGE_PORT").ok();
        std::env::remove_var("FLOWIX_CONSULT_BRIDGE_PORT");
        let responses = run(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"consult_targets","arguments":{}}}"#,
        );
        if let Some(saved) = saved {
            std::env::set_var("FLOWIX_CONSULT_BRIDGE_PORT", saved);
        }
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], json!(true));
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("BRIDGE_UNAVAILABLE"));
    }

    /// P2 非对称闸门纯函数：新增即扩（含子树扩大引入的新字符串），缩小/等价放行。
    #[test]
    fn memory_expansion_detects_additions_and_allows_shrinks() {
        let old = vec!["tag:finance".to_string(), "tag:blog/tech".to_string()];
        // 新增 → 拒。
        assert_eq!(
            memory_expansion(&old, &["tag:finance".into(), "tag:diary".into()]),
            Some("tag:diary".to_string())
        );
        // 子树扩大（新字符串）→ 拒。
        assert_eq!(
            memory_expansion(&old, &["tag:finance".into(), "tag:blog".into()]),
            Some("tag:blog".to_string())
        );
        // 缩小 / 等价换序 → 放行。
        assert_eq!(memory_expansion(&old, &["tag:finance".into()]), None);
        assert_eq!(
            memory_expansion(&old, &["tag:blog/tech".into(), "tag:finance".into()]),
            None
        );
        // 空库起步：任何添加都算扩。
        assert_eq!(
            memory_expansion(&[], &["tag:finance".into()]),
            Some("tag:finance".to_string())
        );
    }

    /// P2 工具面在测试环境（无 .agents 或缺 agent）给出结构化错误。
    #[test]
    fn agent_profile_update_unknown_agent_is_structured_error() {
        let responses = run(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"agent_profile","arguments":{"action":"update","agentId":"agt_missing00","patch":{"system":"x"}}}}"#,
        );
        let message = responses[0]["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("AGENT_NOT_FOUND"), "{message}");
    }

    #[test]
    fn rejects_shell_syntax_without_executing_it() {
        for command in ["show abc; rm -rf ~", "show abc | cat", "show $(whoami)"] {
            let error = execute_command(command, None).unwrap_err();
            assert!(matches!(error, CliError::Usage(_)));
            assert!(error.to_string().contains("shell syntax"));
        }
    }

    #[test]
    fn validates_stdin_contract() {
        assert!(require_stdin(None, "create").is_err());
        assert!(require_stdin(Some("  "), "write").is_err());
        assert!(reject_stdin(Some("unexpected")).is_err());
        assert_eq!(require_stdin(Some("# note"), "create").unwrap(), "# note");
    }
}
