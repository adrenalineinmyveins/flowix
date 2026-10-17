//! F3 技能体系 IPC 命令：技能清单、发布状态与调用统计。

use crate::plugin::set_publish_state as set_state;
use crate::skill_registry::{list_skills, usage_summary, SkillDescriptor, SkillUsageSummary};

#[tauri::command]
pub fn skill_registry_list() -> Result<Vec<SkillDescriptor>, String> {
    list_skills()
}

#[tauri::command]
pub fn skill_set_publish_state(skill_id: String, state: String) -> Result<(), String> {
    set_state(&skill_id, &state)
}

#[tauri::command]
pub fn skill_usage_summary(skill_id: Option<String>) -> Result<Vec<SkillUsageSummary>, String> {
    usage_summary(skill_id.as_deref())
}
