//! flowix-relay：F5 网络中继参考实现（FD-10 M2）。
//! 契约：docs/F5-NETWORK-RELAY.md。

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
        .with_state(state)
}
