//! F6 IPC：脱敏配置读写（`~/.flowix/redaction/config.json`）+ 自定义词表加载
//! （`~/.flowix/redaction/words.txt`，用户手改）+ L1 规则预览。
//!
//! M1 切片：`redaction_preview` 只跑 L1 规则引擎（入参直接给文本）；
//! 完整管线（L0 取材 + L2 蒸馏 + L3 确认门）在 M2/M3 接入，届时
//! `redaction_preview` 升级为 (question, scope, targetLevel) 口径。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flowix_redaction::{self, CustomWord, RedactionOptions, RedactionReport};
use serde::{Deserialize, Serialize};

pub const REDACTION_LEVELS: &[&str] = &["r1", "r2", "r3"];
const SCHEMA_VERSION: u32 = 1;

fn default_level() -> String {
    "r2".to_string()
}

fn default_payload_limit_kb() -> u32 {
    64
}

fn default_confirm_timeout_secs() -> u32 {
    120
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionConfig {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default = "default_level")]
    pub default_level: String,
    #[serde(default)]
    pub level_overrides: BTreeMap<String, String>,
    #[serde(default = "default_payload_limit_kb")]
    pub payload_limit_kb: u32,
    #[serde(default = "default_confirm_timeout_secs")]
    pub confirm_timeout_secs: u32,
    #[serde(default)]
    pub strict_luhn: bool,
    #[serde(default = "default_true")]
    pub rewrite_absolute_time: bool,
    /// FD-8：蒸馏 Agent 类型覆盖。空 = 默认 deepseek-harness；
    /// 非法值在蒸馏派发时报 agent_type_invalid。
    #[serde(default)]
    pub distill_agent_type: String,
    /// F5：平台中继地址（如 https://relay.flowix.cc/v1/consult）。
    /// 空 = 未配置，transmit 返回 RELAY_NOT_CONFIGURED。
    #[serde(default)]
    pub relay_url: String,
    /// F5：本机在中继的收件通道凭据（redaction_inbox_register 写入）。
    #[serde(default)]
    pub device_token: Option<String>,
    /// F5：已注册到中继的本机 agent（与 device_token 配套）。
    #[serde(default)]
    pub registered_agents: Vec<String>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            default_level: default_level(),
            level_overrides: BTreeMap::new(),
            payload_limit_kb: default_payload_limit_kb(),
            confirm_timeout_secs: default_confirm_timeout_secs(),
            strict_luhn: false,
            rewrite_absolute_time: true,
            distill_agent_type: String::new(),
            relay_url: String::new(),
            device_token: None,
            registered_agents: Vec::new(),
        }
    }
}

fn redaction_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction"))
}

fn config_file(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

fn words_file(dir: &Path) -> PathBuf {
    dir.join("words.txt")
}

pub(crate) fn read_config_from(dir: &Path) -> RedactionConfig {
    // 真机点检发现：PowerShell 等工具写出的 UTF-8 BOM 会让 serde_json
    // 解析失败而静默回退默认值（relayUrl 等覆盖丢失）。读入时剥 BOM。
    fs::read_to_string(config_file(dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<RedactionConfig>(raw.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

pub(crate) fn write_config_to(dir: &Path, config: &RedactionConfig) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("CONFIG_DIR_CREATE_FAILED: {e}"))?;
    let raw = serde_json::to_string_pretty(config)
        .map_err(|e| format!("CONFIG_SERIALIZE_FAILED: {e}"))?;
    let target = config_file(dir);
    let tmp = dir.join("config.json.tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
        f.write_all(raw.as_bytes())
            .map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
        f.sync_all().map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
    }
    fs::rename(&tmp, &target).map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
    Ok(())
}

pub(crate) fn read_words_from(dir: &Path) -> Vec<CustomWord> {
    fs::read_to_string(words_file(dir))
        .map(|raw| {
            raw.lines()
                .filter_map(flowix_redaction::parse_word_line)
                .collect()
        })
        .unwrap_or_default()
}

fn validate_config(config: &RedactionConfig) -> Result<(), String> {
    if !REDACTION_LEVELS.contains(&config.default_level.as_str()) {
        return Err("INVALID_DEFAULT_LEVEL".to_string());
    }
    if let Some((skill, level)) = config
        .level_overrides
        .iter()
        .find(|(_, level)| !REDACTION_LEVELS.contains(&level.as_str()))
    {
        return Err(format!("INVALID_LEVEL_OVERRIDE:{skill}:{level}"));
    }
    if !(1..=4096).contains(&config.payload_limit_kb) {
        return Err("INVALID_PAYLOAD_LIMIT".to_string());
    }
    if !(30..=600).contains(&config.confirm_timeout_secs) {
        return Err("INVALID_CONFIRM_TIMEOUT".to_string());
    }
    Ok(())
}

fn load_config() -> Result<RedactionConfig, String> {
    Ok(read_config_from(&redaction_dir()?))
}

fn load_words() -> Result<Vec<CustomWord>, String> {
    Ok(read_words_from(&redaction_dir()?))
}

/// F6 管线（crate::redaction::preview_full）取配置：读不到文件时全默认值。
pub(crate) fn redaction_config_for_pipeline() -> RedactionConfig {
    redaction_dir()
        .map(|dir| read_config_from(&dir))
        .unwrap_or_default()
}

/// F6 管线取自定义词表：文件缺失/不可读时为空表。
pub(crate) fn redaction_words_for_pipeline() -> Vec<CustomWord> {
    redaction_dir()
        .map(|dir| read_words_from(&dir))
        .unwrap_or_default()
}

#[tauri::command]
pub fn redaction_config_get() -> Result<RedactionConfig, String> {
    load_config()
}

#[tauri::command]
pub fn redaction_config_set(config: RedactionConfig) -> Result<RedactionConfig, String> {
    validate_config(&config)?;
    let mut normalized = config;
    normalized.schema_version = SCHEMA_VERSION;
    let dir = redaction_dir()?;
    write_config_to(&dir, &normalized)?;
    Ok(normalized)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionPreview {
    pub report: flowix_redaction::RedactionReport,
    pub word_count: usize,
}

/// M1 诊断切片：文本直跑 L1 规则（不进 L0/L2 管线），供前端调试词表与规则。
#[tauri::command]
pub fn redaction_l1_preview(text: String) -> Result<RedactionPreview, String> {
    if text.trim().is_empty() {
        return Err("TEXT_REQUIRED".to_string());
    }
    let config = load_config()?;
    let words = load_words()?;
    let options = RedactionOptions {
        strict_luhn: config.strict_luhn,
        rewrite_absolute_time: config.rewrite_absolute_time,
        now: config
            .rewrite_absolute_time
            .then(chrono::Local::now)
            .map(|now| now.date_naive()),
    };
    let report = flowix_redaction::redact(&text, &options, &words);
    Ok(RedactionPreview {
        report,
        word_count: words.len(),
    })
}

/// 完整 preview 管线（规格 §5.3）：L0 组装（scope 白名单）→ L1 规则脱敏 →
/// L2 蒸馏（DSH agent 会话）→ L3 复扫回炉。返回未确认态报告；
/// 错误码 scope_empty / payload_too_large / distill_failed /
/// rule_hit_after_distill / placeholder_broken / invalid_level。
///
/// `min_levels`：参与技能/身份卡声明的最低脱敏等级（P1-2 联动），
/// 与 target_level 取 max 后生效；省略 = 仅用户等级。
#[tauri::command]
pub async fn redaction_preview(
    app_handle: tauri::AppHandle,
    cwd: String,
    question: String,
    scope_entry_ids: Vec<String>,
    target_level: String,
    min_levels: Option<Vec<String>>,
    sender_agent_id: Option<String>,
    sender_display_name: Option<String>,
    recipient_agent_id: Option<String>,
    recipient_display_name: Option<String>,
    in_reply_to: Option<String>,
    reply_hops_remaining: Option<u32>,
    blind_relay: Option<bool>,
) -> Result<crate::redaction::RedactionPreviewReport, String> {
    if question.trim().is_empty() {
        return Err("QUESTION_REQUIRED".to_string());
    }
    let root = Path::new(&cwd);
    if !root.is_absolute() || !root.is_dir() {
        return Err("INVALID_CWD".to_string());
    }
    let effective = crate::redaction::effective_target_level(
        &target_level,
        &min_levels.unwrap_or_default(),
    )
    .map_err(|error| error.to_ipc_error())?;
    // F5 wire v1 路由：给定收件人才算网络咨询；缺省 local → manual。
    let parties = recipient_agent_id
        .map(|agent_id| {
            (
                crate::redaction::outbox::AgentParty {
                    agent_id: sender_agent_id.unwrap_or_else(|| "local".to_string()),
                    display_name: sender_display_name.unwrap_or_default(),
                },
                crate::redaction::outbox::AgentParty {
                    agent_id,
                    display_name: recipient_display_name.unwrap_or_default(),
                },
            )
        });
    // 回程信封（对已接受咨询的回答）：inReplyTo + 继承跳数。
    let reply = in_reply_to.map(|receipt_id| {
        let hops = reply_hops_remaining
            .unwrap_or(crate::redaction::outbox::DISCLOSURE_MAX_HOPS.saturating_sub(1));
        (receipt_id, hops)
    });
    crate::redaction::preview_full_inner(
        &app_handle,
        &cwd,
        &question,
        &scope_entry_ids,
        &effective,
        parties,
        reply,
        blind_relay.unwrap_or(false),
    )
    .await
    .map_err(|error| error.to_ipc_error())
}

fn notebook_root(cwd: &str) -> Result<PathBuf, String> {
    let path = Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err("INVALID_CWD".to_string());
    }
    Ok(path.to_path_buf())
}

/// 确认门决定：approved=true → 留档 confirmed；false → cancelled。
/// 留档不存在 / 已结算 → RECEIPT_NOT_FOUND / ALREADY_SETTLED。
#[tauri::command]
pub fn redaction_confirm(
    cwd: String,
    receipt_id: String,
    approved: bool,
) -> Result<crate::redaction::RedactionOutboxRecord, String> {
    let root = notebook_root(&cwd)?;
    crate::redaction::submit_confirmation(&root, &receipt_id, approved)
}

/// 确认门挂起等待（agent 自动发起路径）。超时 → 留档 cancelled +
/// `confirm_timeout:<id>`；拒绝 → cancelled 正常返回。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ConfirmGateResult {
    Confirmed { payload: String },
    Cancelled,
}

#[tauri::command]
pub async fn redaction_confirm_wait(
    cwd: String,
    receipt_id: String,
    timeout_secs: Option<u64>,
) -> Result<ConfirmGateResult, String> {
    let root = notebook_root(&cwd)?;
    let timeout = timeout_secs.unwrap_or_else(|| load_config().unwrap_or_default().confirm_timeout_secs as u64);
    match crate::redaction::wait_for_confirmation(&root, &receipt_id, timeout).await {
        Ok(crate::redaction::ConfirmOutcome::Confirmed(bytes)) => {
            let payload = String::from_utf8(bytes)
                .map_err(|_| "ENVELOPE_NOT_UTF8".to_string())?;
            Ok(ConfirmGateResult::Confirmed { payload })
        }
        Ok(crate::redaction::ConfirmOutcome::Cancelled) => Ok(ConfirmGateResult::Cancelled),
        Err(error) => Err(error.to_ipc_error()),
    }
}

/// 唯一发送出口：只有 confirmed 留档能取出信封字节（与留档 byte-equal）。
/// draft / cancelled / failed / 不存在一律 `UNCONFIRMED_SEND_BLOCKED:*`。
/// F5 网络层就绪后在此出口上接传输，兜底断言保持在本函数。
#[tauri::command]
pub fn redaction_send(cwd: String, receipt_id: String) -> Result<SendPayload, String> {
    let root = notebook_root(&cwd)?;
    let bytes = crate::redaction::take_confirmed_payload(&root, &receipt_id)?;
    let byte_length = bytes.len();
    let payload = String::from_utf8(bytes).map_err(|_| "ENVELOPE_NOT_UTF8".to_string())?;
    Ok(SendPayload {
        payload,
        byte_length,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPayload {
    pub payload: String,
    pub byte_length: usize,
}

/// 出站留档列表（created_at 升序），确认门 UI 与审计页共用。
#[tauri::command]
pub fn redaction_outbox_list(
    cwd: String,
) -> Result<Vec<crate::redaction::RedactionOutboxRecord>, String> {
    let root = notebook_root(&cwd)?;
    Ok(crate::redaction::list_records(&root))
}

/// F5 客户端传输层：把 confirmed 信封从唯一出口投递到配置的平台中继。
/// 中继未配置返回 RELAY_NOT_CONFIGURED；未过确认门沿 UNCONFIRMED_SEND_BLOCKED
/// 兜底；投递失败留档保持 confirmed（take 幂等，可重试）。
#[tauri::command]
pub async fn redaction_transmit(
    cwd: String,
    receipt_id: String,
) -> Result<crate::redaction::RelayDeliverResult, String> {
    let root = notebook_root(&cwd)?;
    let config = load_config()?;
    crate::redaction::deliver_confirmed_envelope(&root, &receipt_id, &config.relay_url).await
}

/// F5 入站：向中继注册本机 agent 收件通道，凭据写入 config。
/// FD-11 v2：有中继账号时裸 slug 归一到 `username/slug` 并以账号 token 注册。
#[tauri::command]
pub async fn redaction_inbox_register(agent_ids: Vec<String>) -> Result<RedactionConfig, String> {
    if agent_ids.is_empty() {
        return Err("AGENT_IDS_EMPTY".to_string());
    }
    let mut config = load_config()?;
    if config.relay_url.trim().is_empty() {
        return Err("RELAY_NOT_CONFIGURED".to_string());
    }
    let account = crate::redaction::account::read_account().filter(|account| {
        crate::redaction::inbound::relay_endpoint(&account.relay_url, "") == crate::redaction::inbound::relay_endpoint(&config.relay_url, "")
    });
    let (agent_ids, account_token) = match &account {
        Some(account) => {
            let normalized = agent_ids
                .iter()
                .map(|agent_id| {
                    if agent_id.contains('/') {
                        agent_id.clone()
                    } else {
                        format!("{}/{}", account.username, agent_id)
                    }
                })
                .collect();
            (normalized, Some(account.device_token.as_str()))
        }
        None => (agent_ids, None),
    };
    let token = crate::redaction::register_device(&config.relay_url, &agent_ids, account_token).await?;
    config.device_token = Some(token);
    config.registered_agents = agent_ids;
    let dir = redaction_dir()?;
    write_config_to(&dir, &config)?;
    Ok(config)
}

/// F5 入站：拉取全部已注册 agent 的收件队列（校验 → 留档 → ack）。
#[tauri::command]
pub async fn redaction_inbox_poll(cwd: String) -> Result<crate::redaction::PollSummary, String> {
    let root = notebook_root(&cwd)?;
    let config = load_config()?;
    let token = config
        .device_token
        .clone()
        .ok_or_else(|| "DEVICE_NOT_REGISTERED".to_string())?;
    let mut summary = crate::redaction::PollSummary::default();
    for agent_id in &config.registered_agents {
        let partial =
            crate::redaction::poll_agent_inbox(&root, &config.relay_url, &token, agent_id).await?;
        summary.stored += partial.stored;
        summary.duplicates += partial.duplicates;
        summary.rejected += partial.rejected;
    }
    Ok(summary)
}

/// F5 入站：留档列表（received_at 降序）。
#[tauri::command]
pub fn redaction_inbox_list(
    cwd: String,
) -> Result<Vec<crate::redaction::InboundRecord>, String> {
    let root = notebook_root(&cwd)?;
    Ok(crate::redaction::list_inbound(&crate::redaction::inbound_dir(&root)))
}

/// F5 入站：接受 / 拒绝一条 pending 咨询（M3c 接受后进 F4 编排）。
#[tauri::command]
pub fn redaction_inbox_set_status(
    cwd: String,
    receipt_id: String,
    accepted: bool,
) -> Result<(), String> {
    let root = notebook_root(&cwd)?;
    let status = if accepted {
        crate::redaction::InboundStatus::Accepted
    } else {
        crate::redaction::InboundStatus::Rejected
    };
    crate::redaction::set_inbound_status(&crate::redaction::inbound_dir(&root), &receipt_id, status)
}

// ----------------------------------------------------------------------
// FD-11 v2：中继账号、目录卡、用量
// ----------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryPreviewResult {
    pub draft: crate::redaction::directory::DirectoryCardDraft,
    /// L1 脱敏前的原文（人审对比）。
    pub original: String,
}

#[tauri::command]
pub fn redaction_account_status(
) -> Result<Option<crate::redaction::account::AccountPublicInfo>, String> {
    Ok(crate::redaction::account::read_account().map(|account| account.public_info()))
}

/// 注册或登录中继账号（密钥即身份）。成功后 config.relay_url 对齐账号中继。
#[tauri::command]
pub async fn redaction_account_register(
    username: String,
    relay_url: String,
) -> Result<crate::redaction::account::AccountPublicInfo, String> {
    let account = crate::redaction::account::register_or_login(&username, &relay_url).await?;
    // 账号与中继绑定：注册成功即把主中继指向账号中继，收发同源。
    let mut config = load_config()?;
    if config.relay_url.trim() != account.relay_url {
        config.relay_url = account.relay_url.clone();
        write_config_to(&redaction_dir()?, &config)?;
    }
    Ok(account.public_info())
}

#[tauri::command]
pub fn redaction_account_forget() -> Result<(), String> {
    crate::redaction::account::forget_account()
}

/// 目录卡发布预览：卡文本过 L1 规则脱敏 → draft 留档待人审。
#[tauri::command]
pub fn redaction_directory_preview(
    cwd: String,
    agent_slug: String,
    display_name: String,
    specialty_tags: Vec<String>,
    visibility: String,
    min_redaction_level: String,
) -> Result<DirectoryPreviewResult, String> {
    let root = notebook_root(&cwd)?;
    let (draft, original) = crate::redaction::directory::publish_preview(
        &root,
        &agent_slug,
        &display_name,
        &specialty_tags,
        &visibility,
        &min_redaction_level,
    )?;
    Ok(DirectoryPreviewResult { draft, original })
}

/// 目录卡确认门：approved → confirmed（可发布）；否则 cancelled。
#[tauri::command]
pub fn redaction_directory_confirm(
    cwd: String,
    draft_id: String,
    approved: bool,
) -> Result<crate::redaction::directory::DirectoryCardDraft, String> {
    let root = notebook_root(&cwd)?;
    crate::redaction::directory::settle_draft(&root, &draft_id, approved)
}

/// 发布 confirmed 卡到中继目录（失败可重试，留档保持 confirmed）。
#[tauri::command]
pub async fn redaction_directory_publish(
    cwd: String,
    draft_id: String,
) -> Result<crate::redaction::directory::DirectoryCardDraft, String> {
    let root = notebook_root(&cwd)?;
    crate::redaction::directory::publish_confirmed(&root, &draft_id).await
}

/// 目录搜索（public 卡）。relay_url 缺省用账号中继。
#[tauri::command]
pub async fn redaction_directory_search(
    relay_url: Option<String>,
    q: String,
    tag: String,
) -> Result<Vec<crate::redaction::directory::DirectoryCard>, String> {
    crate::redaction::directory::search(relay_url.as_deref(), &q, &tag).await
}

/// 单卡详情（public/unlisted 可达；private → CARD_FETCH_REJECTED:404）。
#[tauri::command]
pub async fn redaction_directory_card(
    relay_url: Option<String>,
    user: String,
    agent_slug: String,
) -> Result<crate::redaction::directory::DirectoryCard, String> {
    crate::redaction::directory::card(relay_url.as_deref(), &user, &agent_slug).await
}

/// 本账号中继用量与配额。
#[tauri::command]
pub async fn redaction_relay_usage(
) -> Result<crate::redaction::directory::RelayUsage, String> {
    crate::redaction::directory::relay_usage().await
}

/// FD-11 §5 收端举报一条入站信封（账号 token 鉴权）。
/// Ok(true) = 已受理；Ok(false) = 此前已举报过（幂等）。
#[tauri::command]
pub async fn redaction_report(
    target_agent: String,
    receipt_id: String,
) -> Result<bool, String> {
    crate::redaction::directory::report_abuse(&target_agent, &receipt_id).await
}

/// FD-11 P2-6：近 N 天用量报表（1-90 天）。
#[tauri::command]
pub async fn redaction_usage_summary(
    days: i64,
) -> Result<crate::redaction::directory::UsageSummary, String> {
    crate::redaction::directory::usage_summary(days.clamp(1, 90)).await
}

// ----------------------------------------------------------------------
// FD-11 P2-8：SSE 推送订阅（新信封通知 → 前端自动拉取）
// ----------------------------------------------------------------------

/// 订阅任务句柄 + 运行标志（进程级单订阅，重复 start = 幂等替换）。
static PUSH_TASK: std::sync::OnceLock<
    std::sync::Mutex<Option<(std::sync::Arc<std::sync::atomic::AtomicBool>, tauri::async_runtime::JoinHandle<()>)>>,
> = std::sync::OnceLock::new();

fn push_task() -> &'static std::sync::Mutex<
    Option<(std::sync::Arc<std::sync::atomic::AtomicBool>, tauri::async_runtime::JoinHandle<()>)>,
> {
    PUSH_TASK.get_or_init(|| std::sync::Mutex::new(None))
}

/// 订阅中继 SSE：命中本机注册 agent 的新信封事件时 emit `redaction-inbound`。
/// 断线 5s 退避重连；拉取语义不变（前端收到事件后自行 poll）。
#[tauri::command]
pub async fn redaction_push_start(
    cwd: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    notebook_root(&cwd)?;
    let config = load_config()?;
    let token = config
        .device_token
        .clone()
        .ok_or_else(|| "DEVICE_NOT_REGISTERED".to_string())?;
    if config.relay_url.trim().is_empty() {
        return Err("RELAY_NOT_CONFIGURED".to_string());
    }
    let mut guard = push_task().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((running, handle)) = guard.take() {
        running.store(false, std::sync::atomic::Ordering::SeqCst);
        handle.abort();
    }
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = running.clone();
    let relay_url = config.relay_url.clone();
    let agents = config.registered_agents.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let mut backoff = std::time::Duration::from_secs(5);
        while flag.load(std::sync::atomic::Ordering::SeqCst) {
            match subscribe_once(&relay_url, &token, &agents, &app_handle, &flag).await {
                Ok(()) => return,
                Err(_) => {
                    // 断线重连（退避封顶 60s）；错误不打断订阅生命周期。
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
                }
            }
        }
    });
    *guard = Some((running, handle));
    Ok(())
}

async fn subscribe_once(
    relay_url: &str,
    token: &str,
    agents: &[String],
    app: &tauri::AppHandle,
    flag: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    use futures_util::StreamExt as _;
    use tauri::Emitter as _;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("PUSH_CLIENT_INIT_FAILED: {e}"))?;
    let url = crate::redaction::inbound::relay_endpoint(relay_url, "/v2/events");
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("PUSH_CONNECT_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("PUSH_REJECTED:{}", response.status().as_u16()));
    }
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        if !flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let chunk = chunk.map_err(|e| format!("PUSH_STREAM_FAILED: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        // SSE 事件以空行分隔；只关心 data: 行。
        while let Some(pos) = buffer.find("\n\n") {
            let block: String = buffer.drain(..pos + 2).collect();
            for line in block.lines() {
                let Some(data) = line.strip_prefix("data:") else { continue };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(data.trim()) else {
                    continue;
                };
                let Some(agent_id) = value.get("agentId").and_then(|v| v.as_str()) else {
                    continue;
                };
                if agents.iter().any(|agent| agent == agent_id) {
                    let _ = app.emit("redaction-inbound", serde_json::json!({ "agentId": agent_id }));
                }
            }
        }
    }
    // 流正常结束（服务端关闭）：按断线重连处理。
    Err("PUSH_STREAM_ENDED".to_string())
}

/// 停止 SSE 订阅（收件箱关闭时调用；幂等）。
#[tauri::command]
pub fn redaction_push_stop() -> Result<(), String> {
    let mut guard = push_task().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((running, handle)) = guard.take() {
        running.store(false, std::sync::atomic::Ordering::SeqCst);
        handle.abort();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = read_config_from(dir.path());
        assert_eq!(config, RedactionConfig::default());
        assert_eq!(config.default_level, "r2");
        assert_eq!(config.payload_limit_kb, 64);
        assert_eq!(config.confirm_timeout_secs, 120);
        assert!(!config.strict_luhn);
        assert!(config.rewrite_absolute_time);
    }

    #[test]
    fn config_tolerates_utf8_bom() {
        let dir = tempfile::tempdir().unwrap();
        let config = RedactionConfig {
            relay_url: "http://127.0.0.1:8787/v1".to_string(),
            ..RedactionConfig::default()
        };
        write_config_to(dir.path(), &config).unwrap();
        // 模拟 PowerShell Set-Content 写出的带 BOM 文件：剥 BOM 后仍可读。
        let raw = std::fs::read(config_file(dir.path())).unwrap();
        let bom_raw = [b'\xEF', b'\xBB', b'\xBF'].iter().chain(raw.iter()).cloned().collect::<Vec<u8>>();
        std::fs::write(config_file(dir.path()), bom_raw).unwrap();
        assert_eq!(read_config_from(dir.path()), config);
    }

    #[test]
    fn config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config = RedactionConfig {
            default_level: "r3".to_string(),
            level_overrides: BTreeMap::from([("skill-a".to_string(), "r1".to_string())]),
            payload_limit_kb: 128,
            confirm_timeout_secs: 90,
            strict_luhn: true,
            rewrite_absolute_time: false,
            distill_agent_type: "deepseek-harness".to_string(),
            relay_url: "https://relay.example/v1".to_string(),
            device_token: Some("dev_x".to_string()),
            registered_agents: vec!["agt_b".to_string()],
            schema_version: 0,
        };
        write_config_to(dir.path(), &config).unwrap();
        assert!(config_file(dir.path()).is_file());
        let loaded = read_config_from(dir.path());
        assert_eq!(loaded, config);
    }

    #[test]
    fn config_rejects_invalid_values() {
        let base = RedactionConfig::default();
        assert_eq!(
            validate_config(&RedactionConfig {
                default_level: "r9".to_string(),
                ..base.clone()
            }),
            Err("INVALID_DEFAULT_LEVEL".to_string())
        );
        assert_eq!(
            validate_config(&RedactionConfig {
                level_overrides: BTreeMap::from([("s".to_string(), "x".to_string())]),
                ..base.clone()
            })
            .err()
            .map(|e| e.split(':').next().unwrap().to_string()),
            Some("INVALID_LEVEL_OVERRIDE".to_string())
        );
        assert!(validate_config(&RedactionConfig {
            payload_limit_kb: 0,
            ..base.clone()
        })
        .is_err());
        assert!(validate_config(&RedactionConfig {
            confirm_timeout_secs: 10,
            ..base
        })
        .is_err());
    }

    #[test]
    fn words_file_parsing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_words_from(dir.path()).is_empty());

        fs::write(
            words_file(dir.path()),
            "张三=>张工\n\n李四\n  王五 => 老王 \n",
        )
        .unwrap();
        let words = read_words_from(dir.path());
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].original, "张三");
        assert_eq!(words[0].replacement.as_deref(), Some("张工"));
        assert_eq!(words[1].original, "李四");
        assert_eq!(words[1].replacement, None);
        assert_eq!(words[2].replacement.as_deref(), Some("老王"));
    }

    #[test]
    fn preview_uses_config_and_words() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(words_file(dir.path()), "张三=>张工\n").unwrap();
        let config = RedactionConfig {
            rewrite_absolute_time: false,
            ..RedactionConfig::default()
        };
        write_config_to(dir.path(), &config).unwrap();

        let options = RedactionOptions {
            strict_luhn: config.strict_luhn,
            rewrite_absolute_time: config.rewrite_absolute_time,
            now: None,
        };
        let words = read_words_from(dir.path());
        let report = flowix_redaction::redact("张三的电话13800138000", &options, &words);
        assert_eq!(report.text, "张工的电话⟦PHONE-1⟧");
    }

    #[test]
    fn l1_preview_rejects_empty_text() {
        let err = redaction_l1_preview("   ".to_string()).unwrap_err();
        assert_eq!(err, "TEXT_REQUIRED");
    }
}
