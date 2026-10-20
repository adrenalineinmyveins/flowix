//! flowix-relay：F5 网络中继（FD-10 v1 + FD-11 v2 多租户）。
//! 契约：docs/F5-NETWORK-RELAY.md（v1）、docs/F5-CROSS-USER-NETWORK.md（v2）。

pub mod envelope;
pub mod routes;
pub mod store;

use std::sync::Arc;

pub fn build_router(state: Arc<routes::AppState>) -> axum::Router {
    axum::Router::new()
        .route("/v1/devices", axum::routing::post(routes::register_device))
        .route("/v1/outbox", axum::routing::post(routes::post_outbox))
        .route("/v1/inbox", axum::routing::get(routes::get_inbox))
        .route("/v1/inbox/ack", axum::routing::post(routes::post_ack))
        .route("/v1/health", axum::routing::get(routes::health))
        .route("/v2/accounts", axum::routing::post(routes::post_account))
        .route("/v2/accounts/challenge", axum::routing::post(routes::post_challenge))
        .route("/v2/usage", axum::routing::get(routes::get_usage))
        .route("/v2/directory", axum::routing::get(routes::search_directory))
        .route("/v2/directory/:user/:agent", axum::routing::get(routes::get_directory_card))
        .route("/v2/directory/:user/:agent", axum::routing::put(routes::put_directory_card))
        .with_state(state)
}
