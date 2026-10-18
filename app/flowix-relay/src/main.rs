//! flowix-relay 可执行入口。
//!
//! 用法：`cargo run -p flowix-relay [-- --addr 127.0.0.1:8788 --state relay-state.json]`

use std::path::PathBuf;
use std::sync::Arc;

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let addr = arg_value(&args, "--addr").unwrap_or_else(|| "127.0.0.1:8788".to_string());
    let state_path = arg_value(&args, "--state")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("relay-state.json"));
    let state = Arc::new(flowix_relay::routes::AppState {
        store: flowix_relay::store::Store::persisted(state_path),
    });
    let app = flowix_relay::build_router(state);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|error| panic!("bind {addr} failed: {error}", addr = addr));
    println!("flowix-relay v1 listening on http://{addr}", addr = addr);
    axum::serve(listener, app).await.expect("relay server exited with error");
}
