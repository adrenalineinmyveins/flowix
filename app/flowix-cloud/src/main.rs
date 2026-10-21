//! flowix-cloud 可执行入口。
//!
//! 用法：`cargo run -p flowix-cloud -- [--addr 0.0.0.0:8787 --db ./flowix-cloud.db]`

use std::path::PathBuf;
use std::sync::Arc;

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let addr = arg_value(&args, "--addr").unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let db_path = arg_value(&args, "--db")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("flowix-cloud.db"));
    let db = flowix_cloud::db::CloudDb::open(&db_path)
        .unwrap_or_else(|error| panic!("open db {}: {error}", db_path.display()));
    let state = Arc::new(flowix_cloud::AppState { db });
    let app = flowix_cloud::build_router(state);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|error| panic!("bind {addr} failed: {error}"));
    println!("flowix-cloud (self-hosted) v{} listening on http://{addr}", env!("CARGO_PKG_VERSION"));
    println!("sqlite: {}", db_path.display());
    axum::serve(listener, app)
        .await
        .expect("cloud server exited with error");
}
