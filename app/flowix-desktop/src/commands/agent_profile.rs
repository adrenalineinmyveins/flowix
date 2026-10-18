//! F1 身份卡（Agent Profile）IPC 命令。
//!
//! 身份卡是一篇 `.agents/<agent_id>/profile.md` Markdown，frontmatter 用
//! `flowix_` 前缀系统字段承载，正文是用户可编辑的人设。与
//! `.agents/agents/<id>/agent.md`（子 Agent 配置）是不同概念，本模块只
//! 管理前者。

use std::fs;
use std::path::{Path, PathBuf};

use flowix_core::agent::{generate_agent_id, AgentProfile, AgentState, ProfileError};

/// state.json 路径：`<nb>/.flowix/agents/<id>/state.json`（仅本地）。
fn state_path(root: &Path, agent_id: &str) -> PathBuf {
    root.join(".flowix").join("agents").join(agent_id).join("state.json")
}

/// 读取运行状态；不存在返回 `STATE_NOT_FOUND`。
pub(crate) fn read_state(root: &Path, agent_id: &str) -> Result<AgentState, String> {
    let path = state_path(root, agent_id);
    if !path.exists() {
        return Err("STATE_NOT_FOUND".to_string());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("读取 state.json 失败: {error}"))?;
    serde_json::from_str(&raw).map_err(|error| format!("解析 state.json 失败: {error}"))
}

/// 原子写运行状态。
pub(crate) fn write_state(root: &Path, state: &AgentState) -> Result<(), String> {
    let path = state_path(root, &state.agent_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建 state 目录失败: {error}"))?;
    }
    let content = serde_json::to_string_pretty(state)
        .map_err(|error| format!("序列化 state 失败: {error}"))?;
    crate::config::user::atomic_write_json(&path, &content)
        .map_err(|error| format!("写入 state.json 失败: {error}"))
}

/// 读取运行状态；无文件时返回该 agent 的新建空状态（内部记账用）。
pub(crate) fn read_state_or_new(root: &Path, agent_id: &str) -> AgentState {
    read_state(root, agent_id).unwrap_or_else(|_| AgentState::new(agent_id))
}

fn notebook_root(cwd: &str) -> Result<PathBuf, String> {
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err("笔记本路径必须是绝对路径".to_string());
    }
    reject_symlink(path, "笔记本目录")?;
    let metadata = fs::metadata(path).map_err(|error| format!("无法读取笔记本目录: {error}"))?;
    if !metadata.is_dir() {
        return Err("笔记本路径不是目录".to_string());
    }
    Ok(path.to_path_buf())
}

fn reject_symlink(path: &Path, label: &str) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!("拒绝使用符号链接作为 {label}: {}", path.display()));
    }
    Ok(())
}

fn agents_root(root: &Path) -> PathBuf {
    root.join(".agents")
}

fn profile_path(root: &Path, agent_id: &str) -> PathBuf {
    agents_root(root).join(agent_id).join("profile.md")
}

fn ensure_inside(root: &Path, path: &Path) -> Result<(), String> {
    if !crate::config::path_is_inside(path, root) {
        return Err("Agent Profile 路径超出笔记本范围".to_string());
    }
    Ok(())
}

fn read_profile(root: &Path, agent_id: &str) -> Result<AgentProfile, String> {
    let path = profile_path(root, agent_id);
    reject_symlink(&path, "profile.md")?;
    if !path.exists() {
        return Err("PROFILE_NOT_FOUND".to_string());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("读取 profile.md 失败: {error}"))?;
    let (profile, _body) = AgentProfile::parse(&content).map_err(profile_error)?;
    if profile.agent_id != agent_id {
        return Err("profile.md 的 flowix_agent_id 与目录不一致".to_string());
    }
    Ok(profile)
}

/// 读取 profile.md 原文（供喂养 runner 做读改写）。
pub(crate) fn read_profile_content(root: &Path, agent_id: &str) -> Result<String, String> {
    let path = profile_path(root, agent_id);
    reject_symlink(&path, "profile.md")?;
    fs::read_to_string(&path).map_err(|error| format!("读取 profile.md 失败: {error}"))
}

/// 原子写回 profile.md 内容（供喂养 runner 复用）。
pub(crate) fn write_profile_content(
    root: &Path,
    agent_id: &str,
    content: &str,
) -> Result<(), String> {
    let path = profile_path(root, agent_id);
    ensure_inside(root, &path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "profile.md 缺少父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建 Agent 目录失败: {error}"))?;
    reject_symlink(&path, "profile.md")?;
    let temporary = parent.join(".profile.md.flowix-tmp");
    reject_symlink(&temporary, "profile 临时文件")?;
    fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("写入 profile 临时文件失败: {error}"))?;
    fs::rename(&temporary, &path).map_err(|error| format!("替换 profile.md 失败: {error}"))
}

fn write_profile(root: &Path, profile: &AgentProfile, body: &str) -> Result<(), String> {
    let path = profile_path(root, &profile.agent_id);
    ensure_inside(root, &path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "profile.md 缺少父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建 Agent 目录失败: {error}"))?;
    reject_symlink(&path, "profile.md")?;
    let content = profile.serialize(body);
    let file_name = "profile.md";
    let temporary = parent.join(format!(".{file_name}.flowix-tmp"));
    reject_symlink(&temporary, "profile 临时文件")?;
    fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("写入 profile 临时文件失败: {error}"))?;
    fs::rename(&temporary, &path).map_err(|error| format!("替换 profile.md 失败: {error}"))
}

fn profile_error(error: ProfileError) -> String {
    match error {
        ProfileError::NotProfile => "NOT_A_PROFILE".to_string(),
        ProfileError::MissingAgentId => "MISSING_AGENT_ID".to_string(),
        ProfileError::MissingDisplayName => "MISSING_DISPLAY_NAME".to_string(),
        ProfileError::BioTooLong(limit) => format!("BIO_TOO_LONG:{limit}"),
        ProfileError::InvalidExpertise { index, value } => {
            format!("INVALID_EXPERTISE:{index}:{value}")
        }
        ProfileError::InvalidRedactionLevel(value) => {
            format!("INVALID_REDACTION_LEVEL:{value}")
        }
    }
}

#[tauri::command]
pub fn agent_profile_list(cwd: String) -> Result<Vec<AgentProfile>, String> {
    let root = notebook_root(&cwd)?;
    let dir = agents_root(&root);
    reject_symlink(&dir, ".agents 目录")?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|error| format!("读取 .agents 失败: {error}"))? {
        let entry = entry.map_err(|error| format!("读取 .agents 失败: {error}"))?;
        let agent_dir = entry.path();
        if !agent_dir.is_dir() {
            continue;
        }
        let profile = agent_dir.join("profile.md");
        if !profile.exists() {
            continue;
        }
        let content = fs::read_to_string(&profile)
            .map_err(|error| format!("读取 profile.md 失败: {error}"))?;
        if let Ok((parsed, _)) = AgentProfile::parse(&content) {
            result.push(parsed);
        }
    }
    result.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    Ok(result)
}

#[tauri::command]
pub fn agent_profile_get(cwd: String, agent_id: String) -> Result<AgentProfile, String> {
    let root = notebook_root(&cwd)?;
    read_profile(&root, &agent_id)
}

#[tauri::command]
pub fn agent_profile_create(
    cwd: String,
    display_name: String,
    bio: String,
    expertise: Vec<String>,
    agent_type: String,
) -> Result<AgentProfile, String> {
    let root = notebook_root(&cwd)?;
    let now = chrono::Utc::now().timestamp_millis();
    let profile = AgentProfile {
        agent_id: generate_agent_id(),
        display_name,
        bio,
        expertise,
        agent_type,
        model: None,
        consultable: true,
        redaction_level: None,
        disclosure_scope: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    profile.validate().map_err(profile_error)?;
    write_profile(&root, &profile, "")?;
    Ok(profile)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn agent_profile_update(
    cwd: String,
    agent_id: String,
    display_name: String,
    bio: String,
    expertise: Vec<String>,
    agent_type: String,
    // 隐私边界与模型覆盖：None = 不修改；Some(v) = 整体替换
    // （redaction_level / model 传空串表示清除）。
    consultable: Option<bool>,
    redaction_level: Option<String>,
    disclosure_scope: Option<Vec<String>>,
    model: Option<String>,
) -> Result<AgentProfile, String> {
    let root = notebook_root(&cwd)?;
    let mut existing = read_profile(&root, &agent_id)?;
    existing.display_name = display_name;
    existing.bio = bio;
    existing.expertise = expertise;
    existing.agent_type = agent_type;
    if let Some(consultable) = consultable {
        existing.consultable = consultable;
    }
    if let Some(level) = redaction_level {
        existing.redaction_level = (!level.trim().is_empty()).then(|| level.trim().to_string());
    }
    if let Some(scopes) = disclosure_scope {
        let scopes: Vec<String> = scopes
            .into_iter()
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
            .collect();
        for scope in &scopes {
            if flowix_core::memo_file::normalize_tag_path(scope).is_none() {
                return Err(format!("INVALID_DISCLOSURE_SCOPE:{scope}"));
            }
        }
        existing.disclosure_scope = scopes;
    }
    if let Some(model) = model {
        existing.model = (!model.trim().is_empty()).then(|| model.trim().to_string());
    }
    existing.updated_at = chrono::Utc::now().timestamp_millis();
    existing.validate().map_err(profile_error)?;
    // 保留正文：重新读取原始内容取 body。
    let path = profile_path(&root, &agent_id);
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("读取 profile.md 失败: {error}"))?;
    let (_parsed, body) = AgentProfile::parse(&content).map_err(profile_error)?;
    write_profile(&root, &existing, &body)?;
    Ok(existing)
}

#[tauri::command]
pub fn agent_profile_delete(cwd: String, agent_id: String) -> Result<(), String> {
    let root = notebook_root(&cwd)?;
    let path = profile_path(&root, &agent_id);
    ensure_inside(&root, &path)?;
    if !path.exists() {
        return Err("PROFILE_NOT_FOUND".to_string());
    }
    reject_symlink(&path, "profile.md")?;
    fs::remove_file(&path).map_err(|error| format!("删除 profile.md 失败: {error}"))?;
    if let Some(parent) = path.parent() {
        if fs::read_dir(parent)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(parent);
        }
    }
    // 级联清理：memories.md 与本地运行面（state.json / nurture.json /
    // nurture-log.jsonl）。同步面只剩用户自建的额外文件。
    let memories = root.join(".agents").join(&agent_id).join("memories.md");
    if crate::config::path_is_inside(&memories, &root) {
        let _ = fs::remove_file(memories);
    }
    let local_dir = root.join(".flowix").join("agents").join(&agent_id);
    if crate::config::path_is_inside(&local_dir, &root) && local_dir.is_dir() {
        let _ = fs::remove_dir_all(&local_dir);
    }
    Ok(())
}

/// 读取 Agent 本地运行状态（用量/最近线程）。无状态文件返回
/// `STATE_NOT_FOUND`（新 agent 尚未运行过）。
#[tauri::command]
pub fn agent_state_get(cwd: String, agent_id: String) -> Result<AgentState, String> {
    let root = notebook_root(&cwd)?;
    // profile 必须存在，避免对已删 agent 查询残留状态。
    read_profile(&root, &agent_id)?;
    read_state(&root, &agent_id)
}

/// 手动触发一次喂养（对该 notebook 的每个 profile 各入队一个 manual
/// job，跳过阈值与间隔检查）。异步执行，进度经 nurture-log 可观测。
#[tauri::command]
pub fn nurture_run_now(
    notebook_id: String,
    state: tauri::State<'_, crate::app::state::AppState>,
) -> Result<usize, String> {
    let notebook_root = {
        let memo_file = crate::lock_utils::read_lock(&state.memo_file, "memo_file");
        memo_file
            .get_notebook_config_by_id(&notebook_id)
            .map(|config| PathBuf::from(config.path))
    };
    let Some(notebook_root) = notebook_root else {
        return Err(format!("notebook not found: {notebook_id}"));
    };
    crate::nurture::enqueue_manual(&notebook_id, &notebook_root);
    Ok(crate::nurture::pending_job_count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        (temp, root)
    }

    #[test]
    fn create_then_read_round_trip() {
        let (_temp, root) = temp_root();
        let profile = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "财报专家".into(),
            "一句话简介".into(),
            vec!["finance/财报".into()],
            "codex".into(),
        )
        .unwrap();
        assert!(profile.agent_id.starts_with("agt_"));

        let read = agent_profile_get(
            root.to_string_lossy().into_owned(),
            profile.agent_id.clone(),
        )
        .unwrap();
        assert_eq!(read.display_name, "财报专家");
        assert_eq!(read.expertise, vec!["finance/财报".to_string()]);
    }

    #[test]
    fn list_returns_all_profiles() {
        let (_temp, root) = temp_root();
        agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
        )
        .unwrap();
        agent_profile_create(
            root.to_string_lossy().into_owned(),
            "B".into(),
            String::new(),
            vec![],
            "claude".into(),
        )
        .unwrap();
        let list = agent_profile_list(root.to_string_lossy().into_owned()).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn get_missing_returns_error() {
        let (_temp, root) = temp_root();
        let err = agent_profile_get(root.to_string_lossy().into_owned(), "agt_none".into())
            .unwrap_err();
        assert_eq!(err, "PROFILE_NOT_FOUND");
    }

    #[test]
    fn delete_removes_profile() {
        let (_temp, root) = temp_root();
        let profile = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
        )
        .unwrap();
        agent_profile_delete(root.to_string_lossy().into_owned(), profile.agent_id).unwrap();
        let list = agent_profile_list(root.to_string_lossy().into_owned()).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn invalid_expertise_rejected() {
        let (_temp, root) = temp_root();
        let err = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec!["空格 标签".into()],
            "codex".into(),
        )
        .unwrap_err();
        assert!(err.starts_with("INVALID_EXPERTISE"));
    }

    #[test]
    fn state_round_trip_via_read_write() {
        let (_temp, root) = temp_root();
        let profile = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
        )
        .unwrap();
        // 新 agent 无状态文件。
        assert_eq!(
            read_state(&root, &profile.agent_id).unwrap_err(),
            "STATE_NOT_FOUND"
        );
        // 记账后 round-trip 一致。
        let mut state = read_state_or_new(&root, &profile.agent_id);
        state.record_run("thr_1", 1000);
        state.record_tokens(120, 30);
        write_state(&root, &state).unwrap();
        let read_back = read_state(&root, &profile.agent_id).unwrap();
        assert_eq!(read_back, state);
        assert_eq!(read_back.runs, 1);
        // IPC 层：profile 存在 + state 存在 → 成功返回。
        let via_ipc = agent_state_get(
            root.to_string_lossy().into_owned(),
            profile.agent_id.clone(),
        )
        .unwrap();
        assert_eq!(via_ipc.recent_thread_ids, vec!["thr_1".to_string()]);
    }

    #[test]
    fn agent_state_get_requires_profile() {
        let (_temp, root) = temp_root();
        let err = agent_state_get(root.to_string_lossy().into_owned(), "agt_ghost".into())
            .unwrap_err();
        assert_eq!(err, "PROFILE_NOT_FOUND");
    }

    #[test]
    fn update_privacy_fields_and_model() {
        let (_temp, root) = temp_root();
        let profile = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
        )
        .unwrap();
        let cwd = root.to_string_lossy().into_owned();

        let updated = agent_profile_update(
            cwd.clone(),
            profile.agent_id.clone(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
            Some(false),
            Some("R2".into()),
            Some(vec!["finance/财报".into(), " ".into()]),
            Some("gpt-x".into()),
        )
        .unwrap();
        assert!(!updated.consultable);
        assert_eq!(updated.redaction_level.as_deref(), Some("R2"));
        // 空白项被过滤，合法项保留。
        assert_eq!(updated.disclosure_scope, vec!["finance/财报".to_string()]);
        assert_eq!(updated.model.as_deref(), Some("gpt-x"));

        // 旧调用形态（隐私参数全 None）不修改既有隐私字段。
        let kept = agent_profile_update(
            cwd.clone(),
            profile.agent_id.clone(),
            "A2".into(),
            String::new(),
            vec![],
            "codex".into(),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(!kept.consultable);
        assert_eq!(kept.redaction_level.as_deref(), Some("R2"));
        assert_eq!(kept.disclosure_scope, vec!["finance/财报".to_string()]);

        // 空串清除 redaction_level / model。
        let cleared = agent_profile_update(
            cwd,
            profile.agent_id,
            "A2".into(),
            String::new(),
            vec![],
            "codex".into(),
            None,
            Some(String::new()),
            None,
            Some(String::new()),
        )
        .unwrap();
        assert_eq!(cleared.redaction_level, None);
        assert_eq!(cleared.model, None);
    }

    #[test]
    fn update_rejects_invalid_disclosure_scope() {
        let (_temp, root) = temp_root();
        let profile = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
        )
        .unwrap();
        let err = agent_profile_update(
            root.to_string_lossy().into_owned(),
            profile.agent_id,
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
            None,
            None,
            Some(vec!["空格 标签".into()]),
            None,
        )
        .unwrap_err();
        assert!(err.starts_with("INVALID_DISCLOSURE_SCOPE"));
    }

    #[test]
    fn delete_cascades_memories_and_local_state() {
        let (_temp, root) = temp_root();
        let profile = agent_profile_create(
            root.to_string_lossy().into_owned(),
            "A".into(),
            String::new(),
            vec![],
            "codex".into(),
        )
        .unwrap();
        let agent_id = profile.agent_id.clone();
        // 铺垫 memories.md 与本地运行面文件。
        let memories = root.join(".agents").join(&agent_id).join("memories.md");
        std::fs::write(&memories, "---\nflowix_kind: agent-memories\n---\n\n## 2026-10-17\n- 旧\n").unwrap();
        let mut state = AgentState::new(&agent_id);
        state.record_run("thr_1", 1);
        write_state(&root, &state).unwrap();
        let local_dir = root.join(".flowix").join("agents").join(&agent_id);
        std::fs::write(local_dir.join("nurture.json"), "{}").unwrap();

        agent_profile_delete(root.to_string_lossy().into_owned(), agent_id.clone()).unwrap();

        assert!(!memories.exists());
        assert!(!local_dir.exists());
        // state 也不可达（目录整体删除）。
        assert!(read_state(&root, &agent_id).is_err());
        // profile 列表为空。
        let list = agent_profile_list(root.to_string_lossy().into_owned()).unwrap();
        assert!(list.is_empty());
    }
}
