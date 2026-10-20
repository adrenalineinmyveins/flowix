//! FD-11 v2 目录（F5-CROSS-USER-NETWORK §4）：公开卡发布与查询。
//!
//! 发布纪律与咨询信封同源：卡内容（displayName + 专长标签）出设备前过
//! L1 规则脱敏并经确认门人审；留档 `<notebook>/.flowix/redaction-directory/`
//! 状态机 draft → confirmed / cancelled → published。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DIRECTORY_DIR_NAME: &str = "redaction-directory";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Draft,
    Confirmed,
    Cancelled,
    Published,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryCardDraft {
    pub id: String,
    pub status: CardStatus,
    pub agent_slug: String,
    /// L1 脱敏后的展示名（出设备内容）。
    pub display_name: String,
    /// L1 脱敏后的专长标签。
    pub specialty_tags: Vec<String>,
    /// `public` | `unlisted` | `private`。
    pub visibility: String,
    /// `R1|R2|R3`。
    pub min_redaction_level: String,
    #[serde(default)]
    pub l1_hits: Vec<flowix_redaction::RuleHit>,
    pub created_at: i64,
    #[serde(default)]
    pub settled_at: Option<i64>,
    #[serde(default)]
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryCard {
    pub address: String,
    pub username: String,
    pub agent_slug: String,
    pub display_name: String,
    pub visibility: String,
    #[serde(default)]
    pub card: serde_json::Value,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayUsage {
    pub window_secs: i64,
    pub deliver_used: i64,
    pub deliver_limit: i64,
    pub poll_used: i64,
    pub poll_limit: i64,
}

pub fn directory_dir(notebook_root: &Path) -> PathBuf {
    notebook_root.join(".flowix").join(DIRECTORY_DIR_NAME)
}

fn draft_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn write_draft(root: &Path, draft: &DirectoryCardDraft) -> Result<(), String> {
    let dir = directory_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("CARD_DIR_CREATE_FAILED: {e}"))?;
    let raw = serde_json::to_string_pretty(draft)
        .map_err(|e| format!("CARD_SERIALIZE_FAILED: {e}"))?;
    let target = draft_path(&dir, &draft.id);
    let tmp = dir.join(format!("{}.tmp", draft.id));
    std::fs::write(&tmp, raw).map_err(|e| format!("CARD_WRITE_FAILED: {e}"))?;
    std::fs::rename(&tmp, &target).map_err(|e| format!("CARD_WRITE_FAILED: {e}"))?;
    Ok(())
}

pub fn read_draft(root: &Path, id: &str) -> Result<DirectoryCardDraft, String> {
    let path = draft_path(&directory_dir(root), id);
    let bytes =
        std::fs::read(&path).map_err(|_| format!("CARD_DRAFT_NOT_FOUND: {id}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("CARD_DRAFT_INVALID: {e}"))
}

/// 发布预览：卡文本（名称 + 标签）过 L1 规则脱敏 → draft 留档待人审。
/// 返回 (draft, 脱敏前原文)——UI 对比展示人审依据。
pub fn publish_preview(
    notebook_root: &Path,
    agent_slug: &str,
    display_name: &str,
    specialty_tags: &[String],
    visibility: &str,
    min_redaction_level: &str,
) -> Result<(DirectoryCardDraft, String), String> {
    let slug = agent_slug.trim().to_lowercase();
    if slug.is_empty()
        || slug.len() > 32
        || !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || slug.starts_with('-')
    {
        return Err("AGENT_SLUG_INVALID".to_string());
    }
    if !matches!(visibility, "public" | "unlisted" | "private") {
        return Err("VISIBILITY_INVALID".to_string());
    }
    if !matches!(min_redaction_level, "R1" | "R2" | "R3") {
        return Err("MIN_LEVEL_INVALID".to_string());
    }
    if specialty_tags.len() > 8
        || specialty_tags.iter().any(|tag| tag.trim().is_empty() || tag.len() > 32)
    {
        return Err("TAGS_INVALID".to_string());
    }

    let config = crate::commands::redaction::redaction_config_for_pipeline();
    let words = crate::commands::redaction::redaction_words_for_pipeline();
    let options = flowix_redaction::RedactionOptions {
        strict_luhn: config.strict_luhn,
        rewrite_absolute_time: config.rewrite_absolute_time,
        now: if config.rewrite_absolute_time {
            Some(chrono::Local::now().date_naive())
        } else {
            None
        },
    };
    let original = format!(
        "{}|{}",
        display_name.trim(),
        specialty_tags.iter().map(|tag| tag.trim()).collect::<Vec<_>>().join(",")
    );
    let report = flowix_redaction::redact(&original, &options, &words);
    let mut parts = report.text.splitn(2, '|');
    let redacted_name = parts.next().unwrap_or_default().trim().to_string();
    let redacted_tags: Vec<String> = parts
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect();
    if redacted_name.is_empty() {
        return Err("DISPLAY_NAME_EMPTIED_BY_REDACTION".to_string());
    }

    let draft = DirectoryCardDraft {
        id: format!("dir_{}", &Uuid::new_v4().simple().to_string()[..12]),
        status: CardStatus::Draft,
        agent_slug: slug,
        display_name: redacted_name,
        specialty_tags: redacted_tags,
        visibility: visibility.to_string(),
        min_redaction_level: min_redaction_level.to_string(),
        l1_hits: report.hits,
        created_at: now_unix(),
        settled_at: None,
        published_at: None,
    };
    write_draft(notebook_root, &draft)?;
    Ok((draft, original))
}

/// 确认门：approved → confirmed；否则 cancelled。
pub fn settle_draft(
    notebook_root: &Path,
    id: &str,
    approved: bool,
) -> Result<DirectoryCardDraft, String> {
    let mut draft = read_draft(notebook_root, id)?;
    if draft.status != CardStatus::Draft {
        return Err(format!("CARD_ALREADY_SETTLED: {id}"));
    }
    draft.status = if approved { CardStatus::Confirmed } else { CardStatus::Cancelled };
    draft.settled_at = Some(now_unix());
    write_draft(notebook_root, &draft)?;
    Ok(draft)
}

async fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("CARD_CLIENT_INIT_FAILED: {e}"))
}

fn default_relay_url() -> Result<String, String> {
    let Some(account) = super::account::read_account() else {
        return Err("ACCOUNT_NOT_CONFIGURED".to_string());
    };
    Ok(account.relay_url)
}

/// 发布 confirmed 卡到中继（账号 token 鉴权；失败可重试，留档保持 confirmed）。
pub async fn publish_confirmed(notebook_root: &Path, id: &str) -> Result<DirectoryCardDraft, String> {
    let account = super::account::read_account().ok_or("ACCOUNT_NOT_CONFIGURED")?;
    let mut draft = read_draft(notebook_root, id)?;
    if draft.status != CardStatus::Confirmed {
        return Err(format!("CARD_UNCONFIRMED_BLOCKED: {id}"));
    }
    let client = http_client().await?;
    let url = super::inbound::relay_endpoint(
        &account.relay_url,
        &format!("/v2/directory/{}/{}", account.username, draft.agent_slug),
    );
    let response = client
        .put(url)
        .bearer_auth(&account.device_token)
        .json(&serde_json::json!({
            "displayName": draft.display_name,
            "specialtyTags": draft.specialty_tags,
            "minRedactionLevel": draft.min_redaction_level,
            "publicKey": account.public_key,
            "visibility": draft.visibility,
        }))
        .send()
        .await
        .map_err(|e| format!("CARD_PUBLISH_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("CARD_PUBLISH_REJECTED:{}", response.status().as_u16()));
    }
    let _ = response.json::<serde_json::Value>().await;
    draft.status = CardStatus::Published;
    draft.published_at = Some(now_unix());
    write_draft(notebook_root, &draft)?;
    Ok(draft)
}

/// 目录搜索（public 卡）。relay_url 缺省用账号中继。
pub async fn search(relay_url: Option<&str>, q: &str, tag: &str) -> Result<Vec<DirectoryCard>, String> {
    let base = match relay_url {
        Some(url) => url.to_string(),
        None => default_relay_url()?,
    };
    let client = http_client().await?;
    let url = super::inbound::relay_endpoint(&base, "/v2/directory");
    let mut request = client.get(url);
    if !q.trim().is_empty() {
        request = request.query(&[("q", q.trim())]);
    }
    if !tag.trim().is_empty() {
        request = request.query(&[("tag", tag.trim())]);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("CARD_SEARCH_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("CARD_SEARCH_REJECTED:{}", response.status().as_u16()));
    }
    let body: serde_json::Value =
        response.json().await.map_err(|e| format!("CARD_SEARCH_DECODE_FAILED: {e}"))?;
    let cards = body["cards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|card| serde_json::from_value::<DirectoryCard>(card).ok())
        .collect();
    Ok(cards)
}

/// 单卡详情（public/unlisted 可达）。
pub async fn card(relay_url: Option<&str>, user: &str, slug: &str) -> Result<DirectoryCard, String> {
    let base = match relay_url {
        Some(url) => url.to_string(),
        None => default_relay_url()?,
    };
    let client = http_client().await?;
    let url = super::inbound::relay_endpoint(&base, &format!("/v2/directory/{user}/{slug}"));
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("CARD_FETCH_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("CARD_FETCH_REJECTED:{}", response.status().as_u16()));
    }
    response
        .json::<DirectoryCard>()
        .await
        .map_err(|e| format!("CARD_FETCH_DECODE_FAILED: {e}"))
}

/// 本账号用量与配额（中继 /v2/usage 透传）。
pub async fn relay_usage() -> Result<RelayUsage, String> {
    let account = super::account::read_account().ok_or("ACCOUNT_NOT_CONFIGURED")?;
    let client = http_client().await?;
    let url = super::inbound::relay_endpoint(&account.relay_url, "/v2/usage");
    let response = client
        .get(url)
        .bearer_auth(&account.device_token)
        .send()
        .await
        .map_err(|e| format!("USAGE_FETCH_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("USAGE_FETCH_REJECTED:{}", response.status().as_u16()));
    }
    let body: serde_json::Value =
        response.json().await.map_err(|e| format!("USAGE_DECODE_FAILED: {e}"))?;
    Ok(RelayUsage {
        window_secs: body["windowSecs"].as_i64().unwrap_or(0),
        deliver_used: body["deliver"]["used"].as_i64().unwrap_or(0),
        deliver_limit: body["deliver"]["limit"].as_i64().unwrap_or(0),
        poll_used: body["poll"]["used"].as_i64().unwrap_or(0),
        poll_limit: body["poll"]["limit"].as_i64().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_redacts_and_settles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (draft, original) = publish_preview(
            root,
            "researcher",
            "张三的财报助手 13800138000",
            &["finance".to_string()],
            "unlisted",
            "R1",
        )
        .unwrap();
        assert_eq!(draft.status, CardStatus::Draft);
        assert_eq!(draft.agent_slug, "researcher");
        assert!(original.contains("13800138000"));
        // L1 命中手机号 → 出设备名占位化。
        assert!(!draft.display_name.contains("13800138000"), "{}", draft.display_name);
        assert!(draft.l1_hits.iter().any(|hit| hit.rule_id.contains("phone")), "{:?}",
            draft.l1_hits);

        let settled = settle_draft(root, &draft.id, true).unwrap();
        assert_eq!(settled.status, CardStatus::Confirmed);
        // 重复结算被拒；拒绝路径置 cancelled。
        assert!(settle_draft(root, &draft.id, true).unwrap_err().contains("ALREADY_SETTLED"));
    }

    #[test]
    fn invalid_inputs_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert_eq!(
            publish_preview(root, "Bad Slug", "n", &[], "public", "R1").unwrap_err(),
            "AGENT_SLUG_INVALID"
        );
        assert_eq!(
            publish_preview(root, "ok", "n", &[], "hidden", "R1").unwrap_err(),
            "VISIBILITY_INVALID"
        );
        assert_eq!(
            publish_preview(root, "ok", "n", &[], "public", "R9").unwrap_err(),
            "MIN_LEVEL_INVALID"
        );
    }
}
