//! F1 身份卡（Agent Profile）IPC 命令。
//!
//! 身份卡是一篇 `.agents/<agent_id>/profile.md` Markdown，frontmatter 用
//! `flowix_` 前缀系统字段承载，正文是用户可编辑的人设。与
//! `.agents/agents/<id>/agent.md`（子 Agent 配置）是不同概念，本模块只
//! 管理前者。

use std::fs;
use std::path::{Path, PathBuf};

use flowix_core::agent::{generate_agent_id, AgentProfile, ProfileError};

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
pub fn agent_profile_update(
    cwd: String,
    agent_id: String,
    display_name: String,
    bio: String,
    expertise: Vec<String>,
    agent_type: String,
) -> Result<AgentProfile, String> {
    let root = notebook_root(&cwd)?;
    let mut existing = read_profile(&root, &agent_id)?;
    existing.display_name = display_name;
    existing.bio = bio;
    existing.expertise = expertise;
    existing.agent_type = agent_type;
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
    Ok(())
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
}
