//! F3 技能体系 IPC 命令：技能清单、发布状态与调用统计。

use crate::plugin::set_publish_state as set_state;
use crate::skill_registry::{list_skills_with_workspace, usage_summary, SkillDescriptor, SkillUsageSummary};

/// AGENT-SKILLS：`cwd`（笔记本根目录）可选——传入时并入该笔记本
/// `.agents/skills/` 的 workspace 技能；缺省仅插件技能（旧调用方兼容）。
#[tauri::command]
pub fn skill_registry_list(cwd: Option<String>) -> Result<Vec<SkillDescriptor>, String> {
    list_skills_with_workspace(cwd.as_deref().map(std::path::Path::new))
}

#[tauri::command]
pub fn skill_set_publish_state(skill_id: String, state: String) -> Result<(), String> {
    set_state(&skill_id, &state)
}

#[tauri::command]
pub fn skill_usage_summary(skill_id: Option<String>) -> Result<Vec<SkillUsageSummary>, String> {
    usage_summary(skill_id.as_deref())
}
