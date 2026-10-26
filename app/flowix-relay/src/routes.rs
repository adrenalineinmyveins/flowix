//! HTTP 端点（v1 契约 §4 + FD-11 v2 账号/配额）。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};

use crate::envelope::{validate_structure, RejectReason, WireEnvelope};
use crate::store::{split_namespaced, Store};

/// 配额（FD-11 §5 默认值；测试可注入小值）。
/// 「已认证」= 投递请求带 Bearer deviceToken（M4 信封签名落地后按 keyId 精确化）。
#[derive(Clone, Copy)]
pub struct QuotaConfig {
    pub window_secs: i64,
    pub deliver_authenticated: i64,
    pub deliver_anonymous: i64,
    pub poll: i64,
    /// 目录卡写入（FD-11 §5：10 次/天）。
    pub directory_write: i64,
    pub directory_window_secs: i64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            window_secs: 3600,
            deliver_authenticated: 30,
            deliver_anonymous: 3,
            poll: 120,
            directory_write: 10,
            directory_window_secs: 86_400,
        }
    }
}

pub struct AppState {
    pub store: Store,
    pub quotas: QuotaConfig,
    /// FD-11 P2-8：入队通知广播（payload = agentId）。SSE 订阅按
    /// token 拥有的 agent 过滤后推送，客户端据此触发拉取。
    pub events: tokio::sync::broadcast::Sender<String>,
    /// FD-11 P2-7：联邦对端中继 URL 列表（一跳转发；空 = 不参与联邦）。
    pub federation_peers: Vec<String>,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(256);
        Self { store, quotas: QuotaConfig::default(), events, federation_peers: Vec::new() }
    }

    /// 测试注入配额用。
    pub fn with_quotas(store: Store, quotas: QuotaConfig) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(256);
        Self { store, quotas, events, federation_peers: Vec::new() }
    }

    pub fn with_peers(store: Store, peers: Vec<String>) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(256);
        Self {
            store,
            quotas: QuotaConfig::default(),
            events,
            federation_peers: peers.into_iter().filter(|p| !p.trim().is_empty()).map(|p| p.trim_end_matches('/').to_string()).collect(),
        }
    }
}

pub type SharedState = std::sync::Arc<AppState>;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub agent_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponse {
    pub device_token: String,
}

/// v1 匿名注册不变；带 Bearer 账号 token 时 agentIds 归一到 `username/slug`
/// 命名空间（裸 slug 补前缀，带前缀须是自己的 username，否则 403）。
pub async fn register_device(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, Json<RejectResponse>)> {
    let now = now();
    let account = bearer_token(&headers).and_then(|token| state.store.account_of_token(&token));
    let agent_ids = match &account {
        Some((_, username)) => body
            .agent_ids
            .iter()
            .map(|agent_id| match split_namespaced(agent_id) {
                Some((user, _)) if user == username => Ok(agent_id.clone()),
                Some(_) => Err("NAMESPACE_MISMATCH"),
                None => Ok(format!("{username}/{agent_id}")),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|code| (StatusCode::FORBIDDEN, Json(RejectResponse { code })))?,
        None => body.agent_ids,
    };
    let user_id = account.as_ref().map(|(user_id, _)| user_id.as_str());
    Ok(Json(RegisterResponse {
        device_token: state.store.register_device(agent_ids, user_id, now),
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxResponse {
    pub accepted_at: i64,
    pub queue_id: String,
}

/// 结构校验失败 → 422 + code；收件人未注册 → 404 RECIPIENT_UNKNOWN；
/// 超配额 → 429 RATE_LIMITED（FD-11 §5：认证档 30/h，匿名 3/h）。
pub async fn post_outbox(
    State(state): State<SharedState>,
    headers: HeaderMap,
    connect_info: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    Json(envelope): Json<WireEnvelope>,
) -> Result<Json<OutboxResponse>, (StatusCode, Json<RejectResponse>)> {
    if let Err(reason) = validate_structure(&envelope, now()) {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: reason.code() })));
    }
    if !state.store.agent_registered(&envelope.to.agent_id) {
        // FD-11 P2-7：本中继无此 agent → 按联邦配置向对端一跳转发；
        // 全部对端不可达/拒收才回 404（对发起方仍是「不存在」）。
        if let Some(queued) = forward_to_peers(&state, &headers, &envelope).await {
            return Ok(Json(queued));
        }
        return Err((
            StatusCode::NOT_FOUND,
            Json(RejectResponse { code: RejectReason::RecipientUnknown.code() }),
        ));
    }
    // FD-11 §4：private 卡的 agent 拒收跨用户信封（对发起方即「不存在」）。
    if state.store.directory_visibility_of_agent(&envelope.to.agent_id).as_deref() == Some("private")
    {
        return Err((
            StatusCode::NOT_FOUND,
            Json(RejectResponse { code: RejectReason::RecipientUnknown.code() }),
        ));
    }
    // FD-11 §5：带签名信封经 from 目录卡公钥验签，失败拒收；
    // 未发卡（无信任锚）放行——信任锚建设随目录覆盖渐进。
    if let Some(signature) = envelope.signature.clone() {
        if let Some(public_key_b64) =
            state.store.directory_public_key_of_agent(&envelope.from.agent_id)
        {
            let verified = (|| -> Result<(), ()> {
                let key_bytes = BASE64.decode(public_key_b64.trim()).map_err(|_| ())?;
                let key_bytes: &[u8; 32] = key_bytes.as_slice().try_into().map_err(|_| ())?;
                let verifying_key =
                    ed25519_dalek::VerifyingKey::from_bytes(key_bytes).map_err(|_| ())?;
                let signature_bytes = BASE64.decode(signature.value.trim()).map_err(|_| ())?;
                let signature =
                    ed25519_dalek::Signature::from_slice(&signature_bytes).map_err(|_| ())?;
                verifying_key
                    .verify(&crate::envelope::canonical_bytes(&envelope), &signature)
                    .map_err(|_| ())
            })();
            if verified.is_err() {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(RejectResponse { code: "SIGNATURE_INVALID" }),
                ));
            }
        }
    }
    let now = now();
    // 配额主体：带 Bearer（任意 deviceToken）= 认证档；联邦转发
    // （X-Forwarded-From）按 `fed:<第一跳>` 计量（同认证档）；匿名按 IP 降额。
    let (subject, limit) = match bearer_token(&headers) {
        Some(token) => (token, state.quotas.deliver_authenticated),
        None => {
            if let Some(origin) = headers
                .get(FORWARDED_FROM)
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| raw.split(',').next())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                (format!("fed:{origin}"), state.quotas.deliver_authenticated)
            } else {
                let peer = connect_info
                    .map(|axum::extract::ConnectInfo(addr)| addr.ip().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                (format!("ip:{peer}"), state.quotas.deliver_anonymous)
            }
        }
    };
    if state.store.count_usage(&subject, "deliver", now - state.quotas.window_secs) >= limit {
        return Err((StatusCode::TOO_MANY_REQUESTS, Json(RejectResponse { code: "RATE_LIMITED" })));
    }
    let recipient = envelope.to.agent_id.clone();
    let queue_id = state.store.enqueue(&recipient, envelope, now);
    state.store.record_usage(&subject, "deliver", now);
    // P2-8：通知订阅方有新信封（agentId 级，无内容）。
    let _ = state.events.send(recipient);
    Ok(Json(OutboxResponse { accepted_at: now, queue_id }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectResponse {
    pub code: &'static str,
}

// ----------------------------------------------------------------------
// FD-11 P2-7：联邦一跳转发（最小形态）
// ----------------------------------------------------------------------

/// 联邦转发标记头：逗号分隔的「已途径中继」URL 列表（防环 + 计量主体）。
const FORWARDED_FROM: &str = "X-Forwarded-From";

/// 向联邦对端转发信封。任一对端 2xx 即成功（返回伪 queueId `fed:<peer>`）。
/// 已在途径列表中的对端不再转发（一跳语义 + 防环）。
/// 信任模型：转发主体按 `fed:<第一跳>` 计量（配额同认证档）；header 可伪造
/// 的收益上限 = 认证档配额，与注册账号等同，v2 记录在案（F5 §8.2）。
async fn forward_to_peers(
    state: &AppState,
    headers: &HeaderMap,
    envelope: &WireEnvelope,
) -> Option<OutboxResponse> {
    if state.federation_peers.is_empty() {
        return None;
    }
    let visited: Vec<String> = headers
        .get(FORWARDED_FROM)
        .and_then(|value| value.to_str().ok())
        .map(|raw| raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    // 本中继自标识：从环境变量取（无则用 agent 目标前缀占位，测试注入方便）。
    let self_url = std::env::var("FLOWIX_RELAY_SELF_URL").unwrap_or_default();
    let mut next_hops: Vec<String> = Vec::new();
    let mut chain = visited.clone();
    if !self_url.is_empty() {
        chain.push(self_url.clone());
    }
    let chain_value = chain.join(", ");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?;
    for peer in &state.federation_peers {
        if visited.iter().any(|v| v == peer) || *peer == self_url {
            continue; // 已途径：防环。
        }
        next_hops.push(peer.clone());
    }
    for peer in next_hops {
        let url = format!("{peer}/v1/outbox");
        let mut request = client.post(&url).json(envelope);
        if !chain_value.is_empty() {
            request = request.header(FORWARDED_FROM, &chain_value);
        }
        if let Ok(response) = request.send().await {
            if response.status().is_success() {
                return Some(OutboxResponse {
                    accepted_at: now(),
                    queue_id: format!("fed:{peer}"),
                });
            }
        }
    }
    None
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxResponse {
    pub envelopes: Vec<crate::store::QueuedEnvelope>,
}

/// Bearer deviceToken 鉴权；token 不持有该 agent → 403；超 120/h → 429。
pub async fn get_inbox(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMapQuery>,
) -> Result<Json<InboxResponse>, (StatusCode, Json<RejectResponse>)> {
    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    let agent_id = params.agent_id;
    if !state.store.token_owns_agent(&token, &agent_id) {
        return Err((StatusCode::FORBIDDEN, Json(RejectResponse { code: "FORBIDDEN" })));
    }
    let now = now();
    if state.store.count_usage(&token, "poll", now - state.quotas.window_secs) >= state.quotas.poll
    {
        return Err((StatusCode::TOO_MANY_REQUESTS, Json(RejectResponse { code: "RATE_LIMITED" })));
    }
    state.store.record_usage(&token, "poll", now);
    Ok(Json(InboxResponse { envelopes: state.store.inbox(&agent_id, now) }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashMapQuery {
    pub agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckRequest {
    pub agent_id: String,
    pub queue_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckResponse {
    pub removed: usize,
}

pub async fn post_ack(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<AckRequest>,
) -> Result<Json<AckResponse>, (StatusCode, Json<RejectResponse>)> {
    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    if !state.store.token_owns_agent(&token, &body.agent_id) {
        return Err((StatusCode::FORBIDDEN, Json(RejectResponse { code: "FORBIDDEN" })));
    }
    Ok(Json(AckResponse { removed: state.store.ack(&body.agent_id, &body.queue_ids) }))
}

// ----------------------------------------------------------------------
// v2 账号（FD-11 §3/§5/§7：密钥即身份，注册与登录挑战共用一个端点）
// ----------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRequest {
    pub username: String,
    /// base64(标准) 的 32 字节 Ed25519 公钥。
    pub device_public_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountResponse {
    pub user_id: String,
    /// 注册成功即返回；登录挑战场景为 None（须先完成挑战签名）。
    pub device_token: Option<String>,
    pub challenge: Option<String>,
}

/// username 不存在 → 注册；存在且公钥一致 → 发登录挑战；公钥不一致 → 409。
pub async fn post_account(
    State(state): State<SharedState>,
    Json(body): Json<AccountRequest>,
) -> Result<(StatusCode, Json<AccountResponse>), (StatusCode, Json<RejectResponse>)> {
    let username = body.username.trim().to_lowercase();
    if username.len() < 3
        || username.len() > 32
        || !username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || username.starts_with('-')
    {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "USERNAME_INVALID" })));
    }
    let public_key = BASE64
        .decode(body.device_public_key.trim())
        .map_err(|_| (StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "PUBLIC_KEY_INVALID" })))?;
    let public_key: &[u8; 32] =
        public_key.as_slice().try_into().map_err(|_| {
            (StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "PUBLIC_KEY_INVALID" }))
        })?;
    ed25519_dalek::VerifyingKey::from_bytes(public_key)
        .map_err(|_| (StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "PUBLIC_KEY_INVALID" })))?;
    let now = now();
    if let Some((user_id, existing)) = state.store.account_by_username(&username) {
        if existing == public_key {
            let challenge = state.store.issue_challenge(&user_id, now);
            return Ok((
                StatusCode::OK,
                Json(AccountResponse {
                    user_id,
                    device_token: None,
                    challenge: Some(challenge),
                }),
            ));
        }
        return Err((StatusCode::CONFLICT, Json(RejectResponse { code: "USERNAME_TAKEN" })));
    }
    let (user_id, device_token) = state
        .store
        .create_account(&username, public_key, now)
        .ok_or((StatusCode::CONFLICT, Json(RejectResponse { code: "USERNAME_TAKEN" })))?;
    Ok((StatusCode::CREATED, Json(AccountResponse { user_id, device_token: Some(device_token), challenge: None })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeRequest {
    pub user_id: String,
    pub challenge: String,
    /// base64(标准) 的 64 字节 Ed25519 签名（对 challenge 原文）。
    pub signature: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResponse {
    pub device_token: String,
}

pub async fn post_challenge(
    State(state): State<SharedState>,
    Json(body): Json<ChallengeRequest>,
) -> Result<Json<ChallengeResponse>, (StatusCode, Json<RejectResponse>)> {
    let signature = BASE64
        .decode(body.signature.trim())
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "SIGNATURE_INVALID" })))?;
    let token = state
        .store
        .redeem_challenge(&body.user_id, &body.challenge, &signature, now())
        .map_err(|code| (StatusCode::UNAUTHORIZED, Json(RejectResponse { code })))?;
    Ok(Json(ChallengeResponse { device_token: token }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAction {
    pub used: i64,
    pub limit: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub window_secs: i64,
    pub deliver: UsageAction,
    pub poll: UsageAction,
}

pub async fn get_usage(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<UsageResponse>, (StatusCode, Json<RejectResponse>)> {
    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    let since = now() - state.quotas.window_secs;
    Ok(Json(UsageResponse {
        window_secs: state.quotas.window_secs,
        deliver: UsageAction {
            used: state.store.count_usage(&token, "deliver", since),
            limit: state.quotas.deliver_authenticated,
        },
        poll: UsageAction {
            used: state.store.count_usage(&token, "poll", since),
            limit: state.quotas.poll,
        },
    }))
}

// ----------------------------------------------------------------------
// v2 计量报表（FD-11 P2-6：按天聚合 + 当前套餐档；计量不结算）
// ----------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDayRow {
    /// `YYYY-MM-DD`（UTC）。
    pub date: String,
    pub deliver: i64,
    pub poll: i64,
    pub directory: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummaryResponse {
    pub days: Vec<UsageDayRow>,
    /// 当前套餐配额档（小时窗/天窗）。
    pub plan: PlanQuotas,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanQuotas {
    pub deliver_per_hour: i64,
    pub poll_per_hour: i64,
    pub directory_per_day: i64,
}

/// 近 N 天（1-90，默认 7）按天用量报表。与配额同源（usage_events）。
pub async fn get_usage_summary(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<SummaryQuery>,
) -> Result<Json<UsageSummaryResponse>, (StatusCode, Json<RejectResponse>)> {
    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    let days = params.days.unwrap_or(7).clamp(1, 90);
    let since = now() - days * 86_400;
    let mut by_day: std::collections::BTreeMap<i64, UsageDayRow> = Default::default();
    for (day, action, count) in state.store.usage_daily(&token, since) {
        let row = by_day.entry(day).or_insert_with(|| UsageDayRow {
            date: unix_day_to_date(day),
            deliver: 0,
            poll: 0,
            directory: 0,
        });
        match action.as_str() {
            "deliver" => row.deliver += count,
            "poll" => row.poll += count,
            "directory" | "directory_write" => row.directory += count,
            _ => {}
        }
    }
    Ok(Json(UsageSummaryResponse {
        days: by_day.into_values().collect(),
        plan: PlanQuotas {
            deliver_per_hour: state.quotas.deliver_authenticated,
            poll_per_hour: state.quotas.poll,
            directory_per_day: state.quotas.directory_write,
        },
    }))
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    days: Option<i64>,
}

fn unix_day_to_date(day: i64) -> String {
    let secs = day * 86_400;
    let days_since_epoch = secs.div_euclid(86_400);
    // 简化 civil-from-days 算法（Howard Hinnant）。
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

// ----------------------------------------------------------------------
// v2 目录（FD-11 §4：公开卡、三档可见性、搜索）
// ----------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishCardRequest {
    pub display_name: String,
    #[serde(default)]
    pub specialty_tags: Vec<String>,
    /// `R1|R2|R3`：本 agent 接受咨询的最低脱敏等级。
    pub min_redaction_level: String,
    /// base64(标准) 的 32 字节 Ed25519 公钥（信任锚，M4 验签用）。
    pub public_key: String,
    /// `public` | `unlisted` | `private`。
    pub visibility: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardResponse {
    pub address: String,
    pub username: String,
    pub agent_slug: String,
    pub display_name: String,
    pub visibility: String,
    pub card: serde_json::Value,
    pub updated_at: i64,
}

fn card_response(row: crate::store::DirectoryRow) -> CardResponse {
    let card = serde_json::from_str(&row.card_json).unwrap_or(serde_json::Value::Null);
    CardResponse {
        address: format!("{}/{}", row.username, row.agent_slug),
        username: row.username,
        agent_slug: row.agent_slug,
        display_name: row.display_name,
        visibility: row.visibility,
        card,
        updated_at: row.updated_at,
    }
}

/// 发布/更新公开卡：Bearer 账号 token，路径 user 须是自己；卡片字段与
/// 可见性枚举校验；agent 须已注册收件通道；目录写 10/天（§5）。
pub async fn put_directory_card(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::extract::Path((user, slug)): axum::extract::Path<(String, String)>,
    Json(body): Json<PublishCardRequest>,
) -> Result<Json<CardResponse>, (StatusCode, Json<RejectResponse>)> {
    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    let Some((user_id, username)) = state.store.account_of_token(&token) else {
        return Err((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })));
    };
    if user != username {
        return Err((StatusCode::FORBIDDEN, Json(RejectResponse { code: "NAMESPACE_MISMATCH" })));
    }
    let slug = slug.trim().to_lowercase();
    if slug.is_empty()
        || slug.len() > 32
        || !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || slug.starts_with('-')
    {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "AGENT_SLUG_INVALID" })));
    }
    if !matches!(body.visibility.as_str(), "public" | "unlisted" | "private") {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "VISIBILITY_INVALID" })));
    }
    let display_name = body.display_name.trim();
    if display_name.is_empty() || display_name.len() > 64 {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "CARD_INVALID" })));
    }
    if body.specialty_tags.len() > 8 || body.specialty_tags.iter().any(|tag| tag.trim().is_empty() || tag.len() > 32)
    {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "CARD_INVALID" })));
    }
    if !matches!(body.min_redaction_level.as_str(), "R1" | "R2" | "R3") {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "CARD_INVALID" })));
    }
    let public_key = BASE64
        .decode(body.public_key.trim())
        .map_err(|_| (StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "PUBLIC_KEY_INVALID" })))?;
    if public_key.len() != 32 {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "PUBLIC_KEY_INVALID" })));
    }
    if !state.store.agent_registered(&format!("{username}/{slug}")) {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "AGENT_NOT_REGISTERED" })));
    }
    let now = now();
    if state.store.count_usage(&token, "directory_write", now - state.quotas.directory_window_secs)
        >= state.quotas.directory_write
    {
        return Err((StatusCode::TOO_MANY_REQUESTS, Json(RejectResponse { code: "RATE_LIMITED" })));
    }
    let card_json = serde_json::to_string(&serde_json::json!({
        "displayName": display_name,
        "specialtyTags": body.specialty_tags,
        "minRedactionLevel": body.min_redaction_level,
        "publicKey": body.public_key.trim(),
    }))
    .expect("card serializable");
    state.store.upsert_directory_card(
        &user_id,
        &username,
        &slug,
        display_name,
        &body.visibility,
        &card_json,
        now,
    );
    state.store.record_usage(&token, "directory_write", now);
    let row = state
        .store
        .directory_card(&username, &slug)
        .expect("just upserted card readable");
    Ok(Json(card_response(row)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub cards: Vec<CardResponse>,
}

/// 搜索只返回 `public` 卡（unlisted 搜索不可见、持地址可达，§4）。
pub async fn search_directory(
    State(state): State<SharedState>,
    axum::extract::Query(params): axum::extract::Query<SearchQuery>,
) -> Json<SearchResponse> {
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let offset = params.offset.unwrap_or(0).max(0);
    Json(SearchResponse {
        cards: state
            .store
            .search_directory(&params.q, &params.tag, limit, offset)
            .into_iter()
            .map(card_response)
            .collect(),
    })
}

/// 单卡详情：public/unlisted 返回；private 或不存在 → 404。
pub async fn get_directory_card(
    State(state): State<SharedState>,
    axum::extract::Path((user, slug)): axum::extract::Path<(String, String)>,
) -> Result<Json<CardResponse>, (StatusCode, Json<RejectResponse>)> {
    match state.store.directory_card(user.trim(), slug.trim()) {
        Some(row) if row.visibility != "private" => Ok(Json(card_response(row))),
        _ => Err((StatusCode::NOT_FOUND, Json(RejectResponse { code: "CARD_NOT_FOUND" }))),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.to_string())
}

// ----------------------------------------------------------------------
// v2 举报（FD-11 §5/§7：收端一键举报，中继落人工审核队列）
// ----------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    /// 被举报信封的 from agentId（如 `alice/researcher` 或裸 `agt_…`）。
    pub target_agent: String,
    /// 被举报信封的 receiptId（溯源锚点）。
    pub receipt_id: String,
}

/// 举报（需 Bearer 账号 token：举报方身份可追责，防举报滥用）。
/// 同一举报方对同一 receipt 幂等：重复 → 409 REPORT_DUPLICATE。
pub async fn post_report(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<ReportRequest>,
) -> Result<StatusCode, (StatusCode, Json<RejectResponse>)> {
    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    if state.store.account_of_token(&token).is_none() {
        return Err((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })));
    }
    let target = body.target_agent.trim();
    let receipt = body.receipt_id.trim();
    if target.is_empty() || receipt.is_empty() {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(RejectResponse { code: "REPORT_INVALID" })));
    }
    if state.store.insert_report(&token, target, receipt, now()) {
        Ok(StatusCode::CREATED)
    } else {
        Err((StatusCode::CONFLICT, Json(RejectResponse { code: "REPORT_DUPLICATE" })))
    }
}

// ----------------------------------------------------------------------
// v2 SSE 推送（FD-11 P2-8：新信封入队通知，客户端据此触发拉取）
// ----------------------------------------------------------------------
/// SSE 事件流：`data: {"agentId": "alice/researcher"}`。
/// 鉴权 = Bearer deviceToken；仅推送该 token 拥有的 agent 的入队事件。
/// 消费语义不变（at-least-once 拉取 + ack），SSE 只是「该来拉了」的信号。
pub async fn get_events(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, (StatusCode, Json<RejectResponse>)> {
    use tokio_stream::StreamExt as _;

    let token = bearer_token(&headers)
        .ok_or((StatusCode::UNAUTHORIZED, Json(RejectResponse { code: "UNAUTHORIZED" })))?;
    let owned: std::collections::HashSet<String> =
        state.store.agents_of_token(&token).into_iter().collect();
    let stream = tokio_stream::wrappers::BroadcastStream::new(state.events.subscribe())
        .filter_map(move |item| {
            let agent_id = item.ok()?;
            if !owned.contains(&agent_id) {
                return None;
            }
            Some(Ok::<_, std::convert::Infallible>(
                axum::response::sse::Event::default()
                    .data(serde_json::json!({ "agentId": agent_id }).to_string()),
            ))
        });
    let sse = axum::response::Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::default().interval(std::time::Duration::from_secs(20)),
    );
    Ok(sse)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub envelope_version: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok", envelope_version: crate::envelope::ENVELOPE_VERSION })
}
