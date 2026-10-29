//! `flowix-cli` 库入口.
//!
//! CLI 解析、调度、存储操作和 MCP 协议拆在独立模块里，便于分别测试。

pub mod cli;
pub(crate) mod collection_store;
pub mod errors;
pub mod fmt;
pub mod mcp;
pub(crate) mod operation;
pub(crate) mod output;
pub(crate) mod path_store;
pub mod paths;
pub mod plugin;
pub mod store;

mod dispatch;

pub use dispatch::run_cli;
pub use errors::CliError;

/// `FLOWIX_HOME` 是进程级全局 env，所有注入它的测试（path_store / mcp）
/// 必须经同一把锁串行，否则并发测试互踩配置目录。
#[cfg(test)]
pub(crate) mod test_env {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub(crate) fn lock_flowix_home() -> MutexGuard<'static, ()> {
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}
