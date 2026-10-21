//! 云同步跨端会话（mobile runtime）的 provider 配置命令。
//!
//! 配置平文件 `<config_dir>/mobile-agent-provider.json`（API key 不上云），
//! 形状与移动端 `agent_provider.json` 同构（baseUrl / apiKey / model）。

use tauri::State;

use crate::agent_external::mobile::MobileProviderConfig;
use crate::app::state::AppState;

#[tauri::command]
pub async fn mobile_provider_get(
    state: State<'_, AppState>,
) -> Result<MobileProviderConfig, String> {
    state.mobile_chat.provider_config()
}

#[tauri::command]
pub async fn mobile_provider_set(
    config: MobileProviderConfig,
    state: State<'_, AppState>,
) -> Result<MobileProviderConfig, String> {
    state.mobile_chat.set_provider_config(&config)?;
    state.mobile_chat.provider_config()
}
