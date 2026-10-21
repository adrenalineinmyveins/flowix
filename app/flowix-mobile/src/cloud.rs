//! 云端会话装配：SyncManager + refresh token 持久化（安卓实现）。
//!
//! 对应桌面端的职责拆分见 docs/android-app-execution-plan.md：
//! flowix-sync 拥有协议与会话机，本模块只提供 secret 持久化与事件桥接。

use std::path::PathBuf;
use std::sync::Arc;

use flowix_sync::{CloudState, RefreshTokenStore, SyncManager, DEFAULT_CLOUD_API_BASE};

pub const CLOUD_STATE_EVENT: &str = "cloud-state-changed";

/// MVP：refresh token 落 App 私有目录单文件（Android 沙箱自带隔离）。
/// M2 计划迁 Android Keystore 加密。
pub struct PrivateFileTokenStore {
    path: PathBuf,
}

impl PrivateFileTokenStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl RefreshTokenStore for PrivateFileTokenStore {
    fn load(&self) -> Result<Option<String>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(token) => Ok(Some(token)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, token: &str) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, token).map_err(|e| e.to_string())
    }

    fn delete(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 在 App 私有目录上装配 SyncManager 并启动会话恢复服务。
pub fn setup(app: &tauri::AppHandle, data_dir: PathBuf) -> Result<Arc<SyncManager>, flowix_sync::SyncError> {
    // 开发期覆盖：私有目录下存在 cloud_api_base 文件时使用其内容作为 API 地址
    // （adb push 写入，需重启 App 生效）；生产构建走 DEFAULT_CLOUD_API_BASE。
    let api_base = std::fs::read_to_string(data_dir.join("cloud_api_base"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUD_API_BASE.to_string());
    let token_store = Arc::new(PrivateFileTokenStore::new(
        data_dir.join("cloud_refresh_token"),
    ));
    let handle = app.clone();
    let notifier = Arc::new(move |state: CloudState| {
        use tauri::Emitter;
        let _ = handle.emit(CLOUD_STATE_EVENT, &state);
    });
    let sync = Arc::new(SyncManager::new_with_session_dependencies(
        &api_base,
        data_dir.join("sync.db"),
        token_store,
        notifier,
    )?);
    let service = sync.clone();
    tauri::async_runtime::spawn(async move {
        service.run_session_service().await;
    });
    Ok(sync)
}
