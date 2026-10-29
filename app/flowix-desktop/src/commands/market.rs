//! AGENT-MARKET：身份卡市场 IPC（M1 我的发布 / M3 订阅）。

use std::path::Path;

use crate::redaction::directory::{self, MarketMyCard};
use crate::redaction::subscriptions::{self, SubscribedCard};

fn notebook_root(cwd: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err("NOTEBOOK_INVALID".to_string());
    }
    Ok(path.to_path_buf())
}

/// 我的卡发布态聚合：本地身份卡 × `.flowix/redaction-directory/` 关联档
/// （纯本地读；relay 不可达不影响）。
#[tauri::command]
pub fn market_my_cards(cwd: String) -> Result<Vec<MarketMyCard>, String> {
    let root = notebook_root(&cwd)?;
    let profiles = crate::nurture::list_notebook_profiles(&root);
    directory::market_my_cards(&root, &profiles)
}

/// 下架（D-3）：本地生成 private confirmed 克隆档 → 发布覆盖中继卡。
/// 失败时新档留 confirmed，可经 `redaction_directory_publish` 重试。
#[tauri::command]
pub async fn market_unpublish(
    cwd: String,
    agent_id: String,
) -> Result<directory::DirectoryCardDraft, String> {
    let root = notebook_root(&cwd)?;
    let draft = directory::unpublish_draft(&root, agent_id.trim())?;
    directory::publish_confirmed(&root, &draft.id).await
}

// ----------------------------------------------------------------------
// M3（D-5）：订阅 = 本地关注列表
// ----------------------------------------------------------------------

/// 订阅：拉单卡详情存快照（幂等覆盖）；relayUrl 缺省用账号中继。
#[tauri::command]
pub async fn market_subscribe(
    address: String,
    relay_url: Option<String>,
) -> Result<SubscribedCard, String> {
    subscriptions::subscribe(address.trim(), relay_url.as_deref()).await
}

/// 退订：删除快照条目。
#[tauri::command]
pub fn market_unsubscribe(address: String) -> Result<(), String> {
    subscriptions::unsubscribe(&address)
}

/// 我的订阅快照列表。
#[tauri::command]
pub fn market_subscriptions() -> Vec<SubscribedCard> {
    subscriptions::list()
}

/// 刷新订阅快照（远端 private/不存在 → SUBSCRIPTION_NOT_FOUND 类错误；
/// 网络失败快照不动）。
#[tauri::command]
pub async fn market_refresh_subscription(address: String) -> Result<SubscribedCard, String> {
    subscriptions::refresh(address.trim()).await
}
