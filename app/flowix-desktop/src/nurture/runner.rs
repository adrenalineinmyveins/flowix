//! F2 喂养运行编排：消费调度 job，执行一次完整喂养周期。
//!
//! 流程对照 F2 详细设计 v2.0 §4：取材（水位增量 + Tag scope 收窄）→
//! 蒸馏（一次性 agent run，复用 plugin 的线程与 stream_end 模式）→
//! 解析校验 → 写回 profile（expertise 全量替换）+ memories 追加 →
//! 水位推进与留痕。失败不推进水位；连续失败 3 次暂停自动触发。

use std::path::{Path, PathBuf};

use flowix_core::agent::AgentProfile;
use tauri::{AppHandle, Listener, Manager};

use crate::agent_wire::{AgentRuntimeConfig, AgentUserMessage, RuntimePathConfig};
use crate::app::state::AppState;
use crate::commands::agent_profile::write_profile_content;
use crate::commands::agent::runtime::start_plugin_chat;
use crate::config::access::AgentAccessKind;
use crate::nurture::{
    apply_nurture_output, build_nurture_prompt, collect_materials, parse_nurture_output,
    pop_next_job, NurtureLogLine, NurtureState,
};

const INTERVAL_MIN_MS: i64 = 7 * 24 * 3600 * 1000;
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const DISTILL_TIMEOUT_SECS: u64 = 900;
const DEFAULT_AGENT_TYPE: &str = "deepseek-harness";
const MATERIAL_LIMIT_KB: usize = 200;
const MATERIAL_ROW_LIMIT: usize = 200;

/// 启动喂养工作循环。由 bootstrap 调用一次。
pub fn spawn_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let job = pop_next_job();
            let Some(job) = job else {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            };
            if let Err(error) =
                run_nurture_once(&app, &job.notebook_id, &job.agent_id, job.manual).await
            {
                tracing::warn!(
                    notebook_id = %job.notebook_id,
                    agent_id = %job.agent_id,
                    "nurture run failed: {error}"
                );
            }
        }
    });
}

/// 执行一次喂养周期（含失败计数与留痕）。
///
/// 返回 Ok(outcome)：succeeded / skipped:*；Err(reason) 仅在真实失败时。
pub(crate) async fn run_nurture_once(
    app: &AppHandle,
    notebook_id: &str,
    agent_id: &str,
    manual: bool,
) -> Result<String, String> {
    match run_inner(app, notebook_id, agent_id, manual).await {
        Ok(outcome) => Ok(outcome),
        Err(reason) => {
            record_failure(app, notebook_id, agent_id, &reason);
            Err(reason)
        }
    }
}

async fn run_inner(
    app: &AppHandle,
    notebook_id: &str,
    agent_id: &str,
    manual: bool,
) -> Result<String, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "app state unavailable".to_string())?;

    // 1. 解析 notebook 根路径与目标 profile。
    let (notebook_root, tag_paths) = {
        let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
        let config = memo_file
            .get_notebook_config_by_id(notebook_id)
            .ok_or_else(|| format!("notebook not found: {notebook_id}"))?;
        let root = PathBuf::from(config.path);
        let tag_paths: Vec<String> = state
            .agent_access
            .get_config()
            .entries
            .into_iter()
            .filter(|entry| {
                entry.enabled
                    && entry.kind == AgentAccessKind::Tag
                    && entry.tag_path.is_some()
            })
            .filter_map(|entry| entry.tag_path)
            .collect();
        (root, tag_paths)
    };

    // 1b. 取 job 指定 agent 的 profile（多 agent 映射：每 job 一个 agent）。
    let profile_content = match crate::commands::agent_profile::read_profile_content(
        &notebook_root,
        agent_id,
    ) {
        Ok(content) => Some(content),
        // 兼容旧 job（nurture: 前缀占位）：回退第一个 profile。
        Err(_) => find_first_profile(&notebook_root)
            .ok()
            .flatten()
            .map(|(_, content)| content),
    };
    let Some(profile_content) = profile_content else {
        return skip_and_log(app, notebook_id, Some(agent_id), "no_profile");
    };
    let (profile, _) = AgentProfile::parse(&profile_content)
        .map_err(|e| format!("profile_parse_failed: {e}"))?;

    // 2. 间隔与暂停检查（手动触发豁免）。
    let mut nurture_state = read_nurture_state(&notebook_root, &profile.agent_id);
    let now = chrono::Utc::now().timestamp_millis();
    if !manual {
        if nurture_state.paused {
            return skip_and_log(app, notebook_id, Some(&profile.agent_id), "paused");
        }
        if let Some(last) = nurture_state.last_run_at {
            if now - last < INTERVAL_MIN_MS {
                return skip_and_log(
                    app,
                    notebook_id,
                    Some(&profile.agent_id),
                    "interval_not_elapsed",
                );
            }
        }
    }

    // 3. 取材（水位增量 + Tag scope 收窄）。
    let (materials, truncated) = {
        let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
        let rows = memo_file
            .list_note_materials_updated_after(
                notebook_id,
                &tag_paths,
                nurture_state.watermark,
                MATERIAL_ROW_LIMIT,
            )
            .map_err(|e| format!("query materials: {e}"))?;
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|row| {
                !row.relative_path.starts_with(".agents/")
                    && !row.relative_path.starts_with(".flowix/")
            })
            .collect();
        let root = notebook_root.clone();
        collect_materials(rows, move |relative| {
            let path = flowix_core::memo_file::notebook_path_from_relative(&root, relative).ok()?;
            std::fs::read_to_string(path)
                .ok()
                .map(|content| flowix_core::memo_file::extract_body_content(&content).to_string())
        }, MATERIAL_LIMIT_KB)
    };
    if materials.is_empty() {
        return skip_and_log(app, notebook_id, Some(&profile.agent_id), "no_material");
    }

    // 4. 蒸馏：一次性 agent run。
    let memories = read_memories(&notebook_root, &profile.agent_id);
    let all_tags = {
        let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
        memo_file
            .read_notebook_tag_paths(Some(notebook_id))
            .unwrap_or_default()
    };
    let prompt = build_nurture_prompt(
        &profile.display_name,
        &profile_content,
        &memories,
        &all_tags,
        &materials,
    );
    let agent_type = if profile.agent_type.trim().is_empty() {
        DEFAULT_AGENT_TYPE.to_string()
    } else {
        profile.agent_type.clone()
    };
    let (answer, thread_id_for_state) = run_distill_once(
        app,
        &state,
        &notebook_root,
        &prompt,
        &agent_type,
        "nurture:distill",
    )
    .await?;

    // 5. 解析与校验。
    let output = parse_nurture_output(&answer)?;

    // 6. 写回 profile + memories。
    let updated = apply_nurture_output(&profile_content, &output)?;
    write_profile_content(&notebook_root, &profile.agent_id, &updated)?;
    append_memories(&notebook_root, &profile.agent_id, &output.memories);

    // 7. 水位推进、运行状态记账与留痕。
    let watermark = materials.iter().map(|m| m.updated_at).max().unwrap_or(now);
    nurture_state.agent_id = profile.agent_id.clone();
    nurture_state.watermark = watermark;
    nurture_state.last_run_at = Some(now);
    nurture_state.consecutive_failures = 0;
    nurture_state.paused = false;
    write_nurture_state(&notebook_root, &profile.agent_id, &nurture_state);
    // F1 运行状态记账：runs +1、lastRunAt、distill thread 进 FIFO。
    // token 用量待 usage 事件接线（F4），当前不计。
    {
        let mut agent_state =
            crate::commands::agent_profile::read_state_or_new(&notebook_root, &profile.agent_id);
        agent_state.record_run(&thread_id_for_state, now);
        let _ = crate::commands::agent_profile::write_state(&notebook_root, &agent_state);
    }
    append_log_line(
        &notebook_root,
        &profile.agent_id,
        NurtureLogLine {
            ts: now,
            kind: "succeeded".into(),
            reason: None,
            material_count: materials.len(),
            truncated,
        },
    );
    Ok(format!(
        "succeeded: agent={} materials={} truncated={}",
        profile.agent_id,
        materials.len(),
        truncated
    ))
}

/// 找笔记本 `.agents/*/profile.md` 中第一个可解析的 profile。
fn find_first_profile(root: &Path) -> Result<Option<(AgentProfile, String)>, String> {
    let agents_dir = root.join(".agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let profile_path = entry.path().join("profile.md");
        if !profile_path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&profile_path) else {
            continue;
        };
        if let Ok((profile, _)) = AgentProfile::parse(&content) {
            return Ok(Some((profile, content)));
        }
    }
    Ok(None)
}

/// 发起一次蒸馏 agent run 并等待完成，返回 (最终 assistant 文本, threadId)。
/// threadId 供调用方做运行状态记账（线程本身已删除）。
///
/// 线程模式与 `plugin_run` 一致：create → listen stream_end → dispatch →
/// 读输出 → 删除线程。F6 脱敏蒸馏共用（title 区分审计来源）。
pub(crate) async fn run_distill_once(
    app: &AppHandle,
    state: &AppState,
    notebook_root: &Path,
    prompt: &str,
    agent_type: &str,
    title: &str,
) -> Result<(String, String), String> {
    let runtime = crate::agent_external::runtime_registry::ExternalRuntimeKind::parse(agent_type)
        .map_err(|e| format!("agent_type_invalid: {e}"))?;
    let runtime_key = runtime.key().to_string();

    let thread = state
        .thread_manager
        .create_thread(
            crate::agent_types::default_agent_id(),
            title.to_string(),
        )
        .await
        .map_err(|e| format!("create thread: {e}"))?;

    let run_id = format!("nurture-{}", uuid::Uuid::new_v4());
    let notebook_path = notebook_root.to_string_lossy().into_owned();
    let path_config = RuntimePathConfig {
        cwd: Some(notebook_path.clone()),
        workspace_paths: vec![notebook_path.clone()],
        permission_mode: None,
        model: None,
        provider_id: None,
        reasoning_effort: None,
        service_tier: None,
        mode: None,
        code_mode: None,
    };
    let message = AgentUserMessage {
        content: prompt.to_string(),
        llm_content: None,
        image_paths: Vec::new(),
        run_id: Some(run_id.clone()),
        system_reminder_directory: Some(notebook_path.clone()),
        agent_type: Some(runtime_key.clone()),
        runtime_config: Some(AgentRuntimeConfig {
            codex: Some(path_config.clone()),
            claude: Some(path_config.clone()),
            hermes: Some(path_config.clone()),
            opencode: Some(path_config.clone()),
            pi: Some(path_config.clone()),
            deepseek_harness: Some(path_config),
        }),
        permission_mode: None,
        codex_model: None,
        codex_reasoning_effort: None,
        conversation_title: Some(title.to_string()),
    };

    let (stream_end_tx, mut stream_end_rx) = tokio::sync::mpsc::unbounded_channel();
    let listener_run_id = run_id.clone();
    let listener = app.listen("agent-chunk", move |event| {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        if payload.get("run_id").and_then(serde_json::Value::as_str)
            != Some(listener_run_id.as_str())
            || payload.get("kind").and_then(serde_json::Value::as_str) != Some("stream_end")
        {
            return;
        }
        let reason = payload
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let _ = stream_end_tx.send(reason);
    });

    let result = async {
        start_plugin_chat(state, &thread.thread_id, message, app)
            .await
            .map_err(|e| format!("dispatch distill: {e}"))?;
        let reason = tokio::time::timeout(
            std::time::Duration::from_secs(DISTILL_TIMEOUT_SECS),
            stream_end_rx.recv(),
        )
        .await
        .map_err(|_| "run_failed: distill timed out".to_string())?
        .ok_or_else(|| "run_failed: stream closed".to_string())?;
        if let Some(reason) = reason.filter(|value| !value.trim().is_empty()) {
            return Err(format!("run_failed: {reason}"));
        }
        let page = state
            .thread_manager
            .get_external_event_messages_page(&runtime_key, &thread.thread_id, None, 1)
            .await
            .map_err(|e| format!("read output: {e}"))?;
        page.and_then(|page| {
            page.messages
                .into_iter()
                .rev()
                .find(|message| message.role == "assistant")
                .map(|message| message.content)
        })
        .ok_or_else(|| "run_failed: no assistant output".to_string())
        .map(|answer| (answer, thread.thread_id.clone()))
    }
    .await;

    app.unlisten(listener);
    if let Err(error) = state
        .thread_manager
        .delete_thread_with_agent_conversations(&thread.thread_id)
        .await
    {
        tracing::warn!(thread_id = %thread.thread_id, "clean up distill thread failed: {error}");
    }
    result
}

fn read_memories(root: &Path, agent_id: &str) -> String {
    let path = root.join(".agents").join(agent_id).join("memories.md");
    std::fs::read_to_string(path).unwrap_or_else(|_| "（暂无）".to_string())
}

/// 追加新记忆到 memories.md（`## 日期` 分节 + 列表项）。
/// 本轮不实现淘汰与 200 条上限（N3 完整版范围）。
fn append_memories(root: &Path, agent_id: &str, memories: &[String]) {
    if memories.is_empty() {
        return;
    }
    let path = root.join(".agents").join(agent_id).join("memories.md");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut content = existing.clone();
    if !existing.contains("flowix_kind: agent-memories") {
        let header = "---\nflowix_kind: agent-memories\n---\n\n";
        content = format!("{header}{content}");
    }
    let today = chrono::Local::now().format("%Y-%m-%d");
    let mut section = format!("\n## {today}\n");
    for memory in memories {
        let one_line: String = memory
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .take(500)
            .collect();
        section.push_str(&format!("- {one_line}\n"));
    }
    content.push_str(&section);
    let _ = std::fs::write(&path, content);
}

fn read_nurture_state(root: &Path, agent_id: &str) -> NurtureState {
    let path = root
        .join(".flowix")
        .join("agents")
        .join(agent_id)
        .join("nurture.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_nurture_state(root: &Path, agent_id: &str, state: &NurtureState) {
    let dir = root.join(".flowix").join("agents").join(agent_id);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("nurture.json");
    if let Ok(encoded) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, encoded);
    }
}

fn append_log_line(root: &Path, agent_id: &str, line: NurtureLogLine) {
    let dir = root.join(".flowix").join("agents").join(agent_id);
    let path = dir.join("nurture-log.jsonl");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(encoded) = serde_json::to_string(&line) {
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

/// 记录一次 skip 留痕并返回 Ok（skip 是正常路径，不进失败计数）。
fn skip_and_log(
    app: &AppHandle,
    notebook_id: &str,
    agent_id: Option<&str>,
    reason: &str,
) -> Result<String, String> {
    // agent_id 缺失时无法定位 per-agent 日志目录，此时只返回原因。
    if let Some(agent_id) = agent_id {
        if let Some(state) = app.try_state::<AppState>() {
            let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
            if let Some(config) = memo_file.get_notebook_config_by_id(notebook_id) {
                let root = PathBuf::from(config.path);
                append_log_line(
                    &root,
                    agent_id,
                    NurtureLogLine {
                        ts: chrono::Utc::now().timestamp_millis(),
                        kind: "skipped".into(),
                        reason: Some(reason.to_string()),
                        material_count: 0,
                        truncated: 0,
                    },
                );
            }
        }
    }
    Ok(format!("skipped:{reason}"))
}

/// 失败留痕：consecutive_failures +1，达到 3 次置 paused，写 failed 日志。
fn record_failure(app: &AppHandle, notebook_id: &str, agent_id: &str, reason: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let notebook_root = {
        let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
        memo_file
            .get_notebook_config_by_id(notebook_id)
            .map(|config| PathBuf::from(config.path))
    };
    let Some(notebook_root) = notebook_root else {
        return;
    };
    let mut nurture_state = read_nurture_state(&notebook_root, agent_id);
    nurture_state.agent_id = agent_id.to_string();
    nurture_state.consecutive_failures = nurture_state.consecutive_failures.saturating_add(1);
    if nurture_state.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
        nurture_state.paused = true;
    }
    append_log_line(
        &notebook_root,
        agent_id,
        NurtureLogLine {
            ts: chrono::Utc::now().timestamp_millis(),
            kind: "failed".into(),
            reason: Some(reason.to_string()),
            material_count: 0,
            truncated: 0,
        },
    );
    write_nurture_state(&notebook_root, agent_id, &nurture_state);
}
