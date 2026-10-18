//! HTTP 端点（契约 §4）。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::envelope::{validate_structure, RejectReason, WireEnvelope};
use crate::store::Store;

pub struct AppState {
    pub store: Store,
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

pub async fn register_device(
    State(state): State<SharedState>,
    Json(body): Json<RegisterRequest>,
) -> Json<RegisterResponse> {
    Json(RegisterResponse { device_token: state.store.register_device(body.agent_ids) })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxResponse {
    pub accepted_at: i64,
    pub queue_id: String,
}

/// 结构校验失败 → 422 + code；收件人未注册 → 404 RECIPIENT_UNKNOWN。
pub async fn post_outbox(
    State(state): State<SharedState>,
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
    let accepted_at = now();
    let recipient = envelope.to.agent_id.clone();
    let queue_id = state.store.enqueue(&recipient, envelope, accepted_at);
    Ok(Json(OutboxResponse { accepted_at, queue_id }))
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

/// Bearer deviceToken 鉴权；token 不持有该 agent → 403。
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
    Ok(Json(InboxResponse { envelopes: state.store.inbox(&agent_id, now()) }))
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
