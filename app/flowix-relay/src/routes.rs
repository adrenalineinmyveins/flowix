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
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self { store, quotas: QuotaConfig::default() }
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
    // 配额主体：带 Bearer（任意 deviceToken）= 认证档；匿名按来源 IP 降额
    // （M4 信封签名落地后按 keyId 精确化，见 FD-11 §5）。
    let (subject, limit) = match bearer_token(&headers) {
        Some(token) => (token, state.quotas.deliver_authenticated),
        None => {
            let peer = connect_info
                .map(|axum::extract::ConnectInfo(addr)| addr.ip().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            (format!("ip:{peer}"), state.quotas.deliver_anonymous)
        }
    };
    if state.store.count_usage(&subject, "deliver", now - state.quotas.window_secs) >= limit {
        return Err((StatusCode::TOO_MANY_REQUESTS, Json(RejectResponse { code: "RATE_LIMITED" })));
    }
    let recipient = envelope.to.agent_id.clone();
    let queue_id = state.store.enqueue(&recipient, envelope, now);
    state.store.record_usage(&subject, "deliver", now);
    Ok(Json(OutboxResponse { accepted_at: now, queue_id }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectResponse {
    pub code: &'static str,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub envelope_version: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok", envelope_version: crate::envelope::ENVELOPE_VERSION })
}
