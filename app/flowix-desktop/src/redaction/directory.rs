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
    /// FD-12 M4 能力位：`off`（默认）| `R3`（自动回答入站咨询并直发回程）。
    #[serde(default)]
    pub auto_answer: String,
    /// AGENT-MARKET D-2：关联的本地身份卡（None = 旧档/收件箱手填发布）。
    #[serde(default)]
    pub agent_id: Option<String>,
    /// 脱敏前原文（D-4 漂移比对用；旧档缺省不比对）。
    #[serde(default)]
    pub source_name: Option<String>,
    #[serde(default)]
    pub source_tags: Vec<String>,
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
/// `agent_id`：AGENT-MARKET D-2 身份卡关联（None = 收件箱手填发布）。
pub fn publish_preview(
    notebook_root: &Path,
    agent_id: Option<&str>,
    agent_slug: &str,
    display_name: &str,
    specialty_tags: &[String],
    visibility: &str,
    min_redaction_level: &str,
    auto_answer: &str,
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
    // FD-12 M4 能力位（空串/`off` 均归一为小写 off；`R3` 大写）。
    let auto_answer = {
        let raw = auto_answer.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("off") {
            "off".to_string()
        } else {
            raw.to_uppercase()
        }
    };
    if !matches!(auto_answer.as_str(), "off" | "R3") {
        return Err("AUTO_ANSWER_INVALID".to_string());
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
        auto_answer,
        agent_id: agent_id.map(str::trim).filter(|value| !value.is_empty()).map(str::to_string),
        source_name: Some(display_name.trim().to_string()),
        source_tags: specialty_tags.iter().map(|tag| tag.trim().to_string()).filter(|tag| !tag.is_empty()).collect(),
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
            "autoAnswer": if draft.auto_answer.is_empty() { "off" } else { draft.auto_answer.as_str() },
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

// ----------------------------------------------------------------------
// AGENT-MARKET M1：我的发布管理（聚合 / 下架）
// ----------------------------------------------------------------------

/// 我的卡发布态聚合（profile + 关联 draft 状态，契约见 AGENT-MARKET §4）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MarketMyCard {
    pub agent_id: String,
    pub display_name: String,
    pub expertise: Vec<String>,
    /// `none` | `draft` | `confirmed` | `published`（cancelled 不构成发布态）。
    pub publish_state: String,
    pub published_slug: Option<String>,
    pub visibility: Option<String>,
    /// 发布白名单内容（名称/专长）与当前 profile 漂移；旧档无原文不比对。
    pub drifted: bool,
}

fn status_rank(status: &CardStatus) -> u8 {
    match status {
        CardStatus::Published => 3,
        CardStatus::Confirmed => 2,
        CardStatus::Draft => 1,
        CardStatus::Cancelled => 0,
    }
}

fn load_drafts(root: &Path) -> Vec<DirectoryCardDraft> {
    let Ok(entries) = std::fs::read_dir(directory_dir(root)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| std::fs::read(&path).ok())
        .filter_map(|bytes| serde_json::from_slice::<DirectoryCardDraft>(&bytes).ok())
        .collect()
}

/// 某 agent 的有效档：**created_at 最新**的非 cancelled 档——重发/下架
/// 都产生新档，最新档即用户最新意图（下架 private 覆盖旧 published 档
/// 的聚合视图）；无有效档返回 None。
fn latest_effective_draft<'a>(
    drafts: &'a [DirectoryCardDraft],
    agent_id: &str,
) -> Option<&'a DirectoryCardDraft> {
    let mut best: Option<&DirectoryCardDraft> = None;
    for draft in drafts.iter().filter(|d| d.agent_id.as_deref() == Some(agent_id)) {
        if status_rank(&draft.status) == 0 {
            continue;
        }
        if best.is_none_or(|current| draft.created_at >= current.created_at) {
            best = Some(draft);
        }
    }
    best
}

/// 聚合：本地身份卡列表 × 发布档关联（纯本地读，无网络）。
pub fn market_my_cards(
    notebook_root: &Path,
    profiles: &[(String, flowix_core::agent::AgentProfile)],
) -> Result<Vec<MarketMyCard>, String> {
    let drafts = load_drafts(notebook_root);
    let mut cards = Vec::with_capacity(profiles.len());
    for (agent_id, profile) in profiles {
        let effective = latest_effective_draft(&drafts, agent_id);
        let (state, slug, visibility, drifted) = match effective {
            None => ("none", None, None, false),
            Some(draft) => {
                let drifted = matches!(draft.status, CardStatus::Published | CardStatus::Confirmed)
                    && draft.source_name.is_some()
                    && (draft.source_name.as_deref() != Some(profile.display_name.as_str())
                        || {
                            let mut a = draft.source_tags.clone();
                            let mut b = profile.expertise.clone();
                            a.sort();
                            b.sort();
                            a != b
                        });
                let state = match draft.status {
                    CardStatus::Published => "published",
                    CardStatus::Confirmed => "confirmed",
                    CardStatus::Draft => "draft",
                    CardStatus::Cancelled => "none",
                };
                (state, Some(draft.agent_slug.clone()), Some(draft.visibility.clone()), drifted)
            }
        };
        cards.push(MarketMyCard {
            agent_id: agent_id.clone(),
            display_name: profile.display_name.clone(),
            expertise: profile.expertise.clone(),
            publish_state: state.to_string(),
            published_slug: slug.filter(|_| state != "none"),
            visibility: visibility.filter(|_| state != "none"),
            drifted,
        });
    }
    Ok(cards)
}

/// 下架（本地层）：克隆该 agent 当前有效档为 visibility=private 的 confirmed
/// 新档（内容与 L1 结论不变；人审由 UI 下架确认对话框承担，状态机仍走
/// confirmed → publish 单出口）。返回新档 id，供 IPC 层接 `publish_confirmed`。
pub fn unpublish_draft(notebook_root: &Path, agent_id: &str) -> Result<DirectoryCardDraft, String> {
    let drafts = load_drafts(notebook_root);
    let current = latest_effective_draft(&drafts, agent_id)
        .filter(|draft| matches!(draft.status, CardStatus::Published | CardStatus::Confirmed))
        .ok_or_else(|| format!("CARD_NOT_PUBLISHED:{agent_id}"))?;
    let mut next = current.clone();
    next.id = format!("dir_{}", &Uuid::new_v4().simple().to_string()[..12]);
    next.status = CardStatus::Confirmed;
    next.visibility = "private".to_string();
    next.created_at = now_unix();
    next.settled_at = Some(now_unix());
    next.published_at = None;
    write_draft(notebook_root, &next)?;
    Ok(next)
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

/// FD-11 §5 收端举报：POST /v2/reports（账号 token 鉴权）。
/// 幂等：同一 receipt 重复举报返回 Ok(false)（中继 409）。
pub async fn report_abuse(target_agent: &str, receipt_id: &str) -> Result<bool, String> {
    let account = super::account::read_account().ok_or("ACCOUNT_NOT_CONFIGURED")?;
    let client = http_client().await?;
    let url = super::inbound::relay_endpoint(&account.relay_url, "/v2/reports");
    let response = client
        .post(url)
        .bearer_auth(&account.device_token)
        .json(&serde_json::json!({ "targetAgent": target_agent, "receiptId": receipt_id }))
        .send()
        .await
        .map_err(|e| format!("REPORT_REQUEST_FAILED: {e}"))?;
    match response.status().as_u16() {
        201 => Ok(true),
        409 => Ok(false),
        code => Err(format!("REPORT_REJECTED:{code}")),
    }
}

/// FD-11 P2-6 计量报表：近 N 天按天用量 + 当前套餐配额档。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    pub date: String,
    pub deliver: i64,
    pub poll: i64,
    pub directory: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub days: Vec<UsageDay>,
    pub plan: UsagePlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePlan {
    pub deliver_per_hour: i64,
    pub poll_per_hour: i64,
    pub directory_per_day: i64,
}

pub async fn usage_summary(days: i64) -> Result<UsageSummary, String> {
    let account = super::account::read_account().ok_or("ACCOUNT_NOT_CONFIGURED")?;
    let client = http_client().await?;
    let url = super::inbound::relay_endpoint(&account.relay_url, "/v2/usage/summary");
    let response = client
        .get(url)
        .query(&[("days", days.to_string())])
        .bearer_auth(&account.device_token)
        .send()
        .await
        .map_err(|e| format!("SUMMARY_FETCH_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("SUMMARY_FETCH_REJECTED:{}", response.status().as_u16()));
    }
    response
        .json()
        .await
        .map_err(|e| format!("SUMMARY_DECODE_FAILED: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(display_name: &str, expertise: &[&str]) -> flowix_core::agent::AgentProfile {
        flowix_core::agent::AgentProfile {
            agent_id: "agt_m1test".into(),
            display_name: display_name.into(),
            bio: String::new(),
            expertise: expertise.iter().map(|tag| tag.to_string()).collect(),
            agent_type: "deepseek-harness".into(),
            model: None,
            consultable: true,
            redaction_level: None,
            disclosure_scope: Vec::new(),
            system: String::new(),
            tools: Vec::new(),
            skills: Vec::new(),
            metadata: Default::default(),
            memory: Vec::new(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn preview_redacts_and_settles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (draft, original) = publish_preview(
            root,
            Some("agt_m1test"),
            "researcher",
            "张三的财报助手 13800138000",
            &["finance".to_string()],
            "unlisted",
            "R1",
            "R3",
        )
        .unwrap();
        assert_eq!(draft.status, CardStatus::Draft);
        assert_eq!(draft.agent_slug, "researcher");
        assert_eq!(draft.agent_id.as_deref(), Some("agt_m1test"));
        assert_eq!(draft.auto_answer, "R3");
        // D-4：脱敏前原文落档（漂移比对源）。
        assert_eq!(draft.source_name.as_deref(), Some("张三的财报助手 13800138000"));
        assert_eq!(draft.source_tags, vec!["finance".to_string()]);
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
            publish_preview(root, None, "Bad Slug", "n", &[], "public", "R1", "off").unwrap_err(),
            "AGENT_SLUG_INVALID"
        );
        assert_eq!(
            publish_preview(root, None, "ok", "n", &[], "hidden", "R1", "off").unwrap_err(),
            "VISIBILITY_INVALID"
        );
        assert_eq!(
            publish_preview(root, None, "ok", "n", &[], "public", "R9", "off").unwrap_err(),
            "MIN_LEVEL_INVALID"
        );
        // FD-12 M4：能力位枚举校验；空串兼容为 off。
        assert_eq!(
            publish_preview(root, None, "ok", "n", &[], "public", "R1", "R2").unwrap_err(),
            "AUTO_ANSWER_INVALID"
        );
        let (draft, _) = publish_preview(root, None, "ok2", "n", &[], "public", "R1", "").unwrap();
        assert_eq!(draft.auto_answer, "off");
        assert_eq!(draft.agent_id, None);
    }

    #[test]
    fn market_my_cards_aggregates_states_and_drift() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // agt_a：未发布；agt_b：published 且无漂移；agt_c：published 后改名 → drifted。
        let published_b = DirectoryCardDraft {
            id: "dir_b".into(),
            status: CardStatus::Published,
            agent_slug: "helper-b".into(),
            display_name: "B".into(),
            specialty_tags: vec!["finance".into()],
            visibility: "unlisted".into(),
            min_redaction_level: "R1".into(),
            auto_answer: "off".into(),
            agent_id: Some("agt_b".into()),
            source_name: Some("B".into()),
            source_tags: vec!["finance".into()],
            l1_hits: Vec::new(),
            created_at: 10,
            settled_at: Some(11),
            published_at: Some(12),
        };
        let mut drifted_c = published_b.clone();
        drifted_c.id = "dir_c".into();
        drifted_c.agent_id = Some("agt_c".into());
        drifted_c.agent_slug = "helper-c".into();
        drifted_c.source_name = Some("旧名字".into());
        write_draft(root, &published_b).unwrap();
        write_draft(root, &drifted_c).unwrap();

        let profiles = vec![
            ("agt_a".to_string(), profile("A", &[])),
            ("agt_b".to_string(), profile("B", &["finance"])),
            ("agt_c".to_string(), profile("新名字", &["finance"])),
        ];
        let cards = market_my_cards(root, &profiles).unwrap();
        assert_eq!(cards.len(), 3);
        let a = cards.iter().find(|c| c.agent_id == "agt_a").unwrap();
        assert_eq!((a.publish_state.as_str(), a.published_slug.is_none(), a.drifted), ("none", true, false));
        let b = cards.iter().find(|c| c.agent_id == "agt_b").unwrap();
        assert_eq!(
            (b.publish_state.as_str(), b.published_slug.as_deref(), b.visibility.as_deref(), b.drifted),
            ("published", Some("helper-b"), Some("unlisted"), false)
        );
        let c = cards.iter().find(|c| c.agent_id == "agt_c").unwrap();
        assert_eq!(c.publish_state, "published");
        assert!(c.drifted, "改名后应提示漂移");
    }

    #[test]
    fn legacy_draft_without_agent_id_is_ignored_not_crashing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // 旧档（收件箱手填发布）：无 agentId / source 字段。
        let legacy = serde_json::json!({
            "id": "dir_legacy",
            "status": "published",
            "agentSlug": "old",
            "displayName": "旧卡",
            "specialtyTags": [],
            "visibility": "public",
            "minRedactionLevel": "R1",
            "autoAnswer": "off",
            "createdAt": 1
        });
        let dir = directory_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dir_legacy.json"), legacy.to_string()).unwrap();

        let profiles = vec![("agt_a".to_string(), profile("A", &[]))];
        let cards = market_my_cards(root, &profiles).unwrap();
        assert_eq!(cards[0].publish_state, "none", "无关联档不影响状态");
    }

    #[test]
    fn unpublish_draft_creates_private_confirmed_clone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (draft, _) = publish_preview(
            root,
            Some("agt_m1test"),
            "helper",
            "名字",
            &["finance".to_string()],
            "unlisted",
            "R1",
            "off",
        )
        .unwrap();
        settle_draft(root, &draft.id, true).unwrap();
        let mut published = read_draft(root, &draft.id).unwrap();
        published.status = CardStatus::Published;
        published.published_at = Some(99);
        // 模拟真实时间差：发布在前，下架在后（真实链路两动作间隔秒级以上）。
        published.created_at = now_unix() - 10;
        write_draft(root, &published).unwrap();

        let next = unpublish_draft(root, "agt_m1test").unwrap();
        assert_eq!(next.status, CardStatus::Confirmed);
        assert_eq!(next.visibility, "private");
        assert_ne!(next.id, draft.id, "下架产生新档，原档留痕");
        assert_eq!(next.agent_slug, "helper");
        assert_eq!(next.source_name.as_deref(), Some("名字"));

        // 未发布/未确认的 agent 下架被拒。
        assert!(unpublish_draft(root, "agt_missing").unwrap_err().contains("CARD_NOT_PUBLISHED"));
        // 聚合视角（本地层）：最新档生效为 confirmed/private（IPC 层接
        // publish_confirmed 后推进 published/private——本测试不触网）。
        let profiles = vec![("agt_m1test".to_string(), profile("名字", &["finance"]))];
        let cards = market_my_cards(root, &profiles).unwrap();
        assert_eq!(
            (cards[0].publish_state.as_str(), cards[0].visibility.as_deref()),
            ("confirmed", Some("private"))
        );
    }
}
