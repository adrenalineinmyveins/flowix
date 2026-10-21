//! flowix-cloud：Flowix Cloud 自建同步后端（单二进制 + SQLite）。
//!
//! 契约来源：`flowix-sync` 客户端（客户端即规范）。
//! 协议坑清单见 `docs/android-app-execution-plan.md` §9（改本 crate 前必读）。
//!
//! 可独立部署：`cargo run -p flowix-cloud -- --addr 0.0.0.0:8787 --db ./flowix-cloud.db`

pub mod auth;
pub mod blobs;
pub mod db;
pub mod sync;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use flowix_sync::{ApiErrorBody, ApiErrorEnvelope, DataEnvelope};
use std::sync::Arc;

pub struct AppState {
    pub db: db::CloudDb,
}

pub type SharedState = Arc<AppState>;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

/// 统一成功信封 `{"data": ...}`。
pub fn data_reply<T: serde::Serialize>(data: T) -> Response {
    Json(DataEnvelope { data }).into_response()
}

/// 顶层双字段能力信封（`{data, upload}` / `{data, download}`）不再包一层 DataEnvelope。
pub fn raw_reply<T: serde::Serialize>(payload: T) -> Response {
    Json(payload).into_response()
}

/// 统一错误信封 `{"error": {code, message, details?}}`。
pub fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErrorEnvelope {
            error: ApiErrorBody {
                code: code.to_string(),
                message: message.into(),
                details: None,
            },
        }),
    )
        .into_response()
}

/// 最新客户端版本清单（免认证）。部署时用环境变量覆盖：
/// FLOWIX_APP_LATEST_NAME / _CODE / _URL / _NOTES。
pub async fn app_latest() -> Response {
    let version_name =
        std::env::var("FLOWIX_APP_LATEST_NAME").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
    let version_code = std::env::var("FLOWIX_APP_LATEST_CODE").unwrap_or_else(|_| "0".into());
    let download_url = std::env::var("FLOWIX_APP_LATEST_URL").unwrap_or_default();
    let notes = std::env::var("FLOWIX_APP_LATEST_NOTES").unwrap_or_default();
    data_reply(serde_json::json!({
        "versionName": version_name,
        "versionCode": version_code,
        "downloadUrl": download_url,
        "notes": notes,
    }))
}

pub fn build_router(state: SharedState) -> axum::Router {
    use axum::routing::{get, post, put};

    axum::Router::new()
        // ── 认证 ──
        .route("/v1/auth/register", post(auth::register))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/refresh", post(auth::refresh))
        .route("/v1/auth/logout/refresh", post(auth::logout_refresh))
        .route("/v1/auth/me", get(auth::me))
        .route("/v1/auth/apple/challenge", post(auth::not_supported))
        .route("/v1/auth/apple/exchange", post(auth::not_supported))
        .route("/v1/auth/apple/link", post(auth::not_supported))
        .route(
            "/v1/auth/google/desktop/exchange",
            post(auth::not_supported),
        )
        // ── 商品 / 结账 / 权益 ──
        .route("/v2/catalog/products", get(sync::products))
        .route("/v2/billing/checkout", post(sync::checkout_unsupported))
        .route("/v2/entitlements/current", get(auth::entitlements))
        // ── V2 同步 ──
        .route("/v2/sync/status", get(sync::status))
        .route("/v2/sync/changes", get(sync::changes))
        .route("/v2/sync/bootstrap", get(sync::bootstrap))
        .route("/v2/sync/history", get(sync::history))
        .route("/v2/sync/push", post(sync::push))
        // ── blob ──
        .route("/v2/blobs/reservations", post(blobs::reserve))
        .route("/v2/blobs/reservations/:id", put(blobs::upload))
        .route(
            "/v2/blobs/reservations/:id/complete",
            post(blobs::complete),
        )
        .route("/v2/blobs/:hash/access", get(blobs::access))
        .route("/v2/blobs/:hash", get(blobs::download))
        // ── 应用更新（免认证；值由 systemd env 提供）──
        .route("/v2/app/latest", get(app_latest))
        // ── 健康检查 ──
        .route("/healthz", get(|| async { data_reply(serde_json::json!({ "ok": true })) }))
        .fallback(|| async { api_error(StatusCode::NOT_FOUND, "NOT_FOUND", "未知端点") })
        .layer(axum::middleware::from_fn(request_id_layer))
        .with_state(state)
}

/// 每个响应都带 `x-request-id`（客户端会把它附加到错误消息）。
async fn request_id_layer(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let request_id = uuid::Uuid::now_v7().simple().to_string();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-request-id",
        axum::http::HeaderValue::from_str(&request_id)
            .unwrap_or(axum::http::HeaderValue::from_static("unavailable")),
    );
    response
}
