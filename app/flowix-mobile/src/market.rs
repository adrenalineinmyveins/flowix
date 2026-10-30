//! 移动端 Agent 市场（浏览+订阅）与个人 Agent 名片（轻量聚合页后端）。
//!
//! - 市场：relay `/v2/directory` 只读客户端（搜索/列表 public 卡）+
//!   本地订阅快照（`market_subscriptions.json`，仅本机）；发卡仍走桌面
//!   （设计文档开放项：directory 发布管线未迁移）。
//! - 个人 Agent 名片：名称/简介/能力标签，存本机 `agent_card.json` 不上云；
//!   为后续手机端发卡预留素材。

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

/// 未注册网络身份时的默认中继（自建部署；注册后以账号中继为准）。
const DEFAULT_RELAY_URL: &str = "http://8.160.162.251:8788";

/// relay /v2/directory 卡（wire camelCase，与桌面 DirectoryCard 同形）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCard {
    pub address: String,
    pub username: String,
    pub agent_slug: String,
    pub display_name: String,
    /// `public` | `unlisted`
    pub visibility: String,
    /// 卡详情（displayName/minRedactionLevel/specialtyTags/autoAnswer…）。
    #[serde(default)]
    pub card: Value,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionEntry {
    pub address: String,
    pub display_name: String,
    pub subscribed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub bio: String,
    pub skills: Vec<String>,
}

impl Default for AgentCard {
    fn default() -> Self {
        Self {
            name: "我的 Agent".into(),
            bio: String::new(),
            skills: Vec::new(),
        }
    }
}

fn data_file(app: &AppHandle, name: &str) -> Result<std::path::PathBuf, String> {
    use tauri::Manager as _;
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(name))
        .map_err(|e| e.to_string())
}

fn read_json<T: DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn write_json<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    flowix_core::memo_file::atomic_write_bytes(path, raw.as_bytes()).map_err(|e| e.to_string())
}

fn relay_base(app: &AppHandle) -> String {
    crate::redaction::account_relay_url(app).unwrap_or_else(|| DEFAULT_RELAY_URL.into())
}

/// 市场搜索（public 卡；空 query 返回全量）。
#[tauri::command]
pub async fn market_search(app: AppHandle, query: String) -> Result<Vec<MarketCard>, String> {
    let base = relay_base(&app).trim_end_matches('/').to_string();
    let mut url = format!("{base}/v2/directory");
    let trimmed = query.trim();
    if !trimmed.is_empty() {
        url.push_str("?q=");
        url.push_str(&urlencode(trimmed));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("MARKET_CLIENT_INIT_FAILED: {e}"))?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("MARKET_SEARCH_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("MARKET_SEARCH_REJECTED:{}", response.status().as_u16()));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("MARKET_SEARCH_DECODE_FAILED: {e}"))?;
    Ok(body["cards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|card| serde_json::from_value::<MarketCard>(card).ok())
        .collect())
}

/// 本地订阅列表（订阅时间升序）。
#[tauri::command]
pub fn market_subscriptions(app: AppHandle) -> Result<Vec<SubscriptionEntry>, String> {
    Ok(read_json(&data_file(&app, "market_subscriptions.json")?).unwrap_or_default())
}

#[tauri::command]
pub fn market_subscribe(
    app: AppHandle,
    address: String,
    display_name: String,
) -> Result<Vec<SubscriptionEntry>, String> {
    let path = data_file(&app, "market_subscriptions.json")?;
    let mut list: Vec<SubscriptionEntry> = read_json(&path).unwrap_or_default();
    if list.iter().any(|entry| entry.address == address) {
        return Ok(list);
    }
    list.push(SubscriptionEntry {
        address,
        display_name,
        subscribed_at: chrono::Utc::now().timestamp_millis(),
    });
    write_json(&path, &list)?;
    Ok(list)
}

#[tauri::command]
pub fn market_unsubscribe(app: AppHandle, address: String) -> Result<Vec<SubscriptionEntry>, String> {
    let path = data_file(&app, "market_subscriptions.json")?;
    let mut list: Vec<SubscriptionEntry> = read_json(&path).unwrap_or_default();
    list.retain(|entry| entry.address != address);
    write_json(&path, &list)?;
    Ok(list)
}

#[tauri::command]
pub fn agent_card_get(app: AppHandle) -> Result<AgentCard, String> {
    Ok(read_json(&data_file(&app, "agent_card.json")?).unwrap_or_default())
}

#[tauri::command]
pub fn agent_card_set(app: AppHandle, card: AgentCard) -> Result<AgentCard, String> {
    let mut card = card;
    card.name = card.name.trim().to_string();
    if card.name.is_empty() {
        return Err("AGENT_CARD_NAME_EMPTY".into());
    }
    card.skills = card
        .skills
        .iter()
        .map(|skill| skill.trim().to_string())
        .filter(|skill| !skill.is_empty())
        .take(8)
        .collect();
    write_json(&data_file(&app, "agent_card.json")?, &card)?;
    Ok(card)
}

fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_card_parses_wire_shape() {
        let raw = r#"{
            "address": "alice/researcher",
            "username": "alice",
            "agentSlug": "researcher",
            "displayName": "alice-researcher",
            "visibility": "public",
            "card": { "minRedactionLevel": "R1", "specialtyTags": ["检索"] },
            "updatedAt": 1793300000
        }"#;
        let card: MarketCard = serde_json::from_str(raw).expect("wire card parses");
        assert_eq!(card.agent_slug, "researcher");
        assert_eq!(card.card["minRedactionLevel"], "R1");
    }

    #[test]
    fn agent_card_default_and_trim() {
        let card = AgentCard::default();
        assert_eq!(card.name, "我的 Agent");
    }
}
