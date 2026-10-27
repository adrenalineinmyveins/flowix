//! IPC 命令面：认证（M1-3）+ 笔记本/笔记 CRUD（M1-6 提前合入）+ 同步触发。

use std::sync::Arc;

use flowix_core::memo_file::{self};
use flowix_sync::{v2_content_hash, v2_path_note_id, CloudState, LocalChangeKind, SyncManager};
use serde::Serialize;
use tauri::Manager;

use crate::cache::{self, MobileState};
use crate::sync::{self, SyncSummary};

// ── 认证 ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn cloud_login(
    sync: tauri::State<'_, Arc<SyncManager>>,
    email: String,
    password: String,
) -> Result<CloudState, String> {
    sync.login(&email, &password).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_register(
    sync: tauri::State<'_, Arc<SyncManager>>,
    email: String,
    password: String,
    display_name: String,
) -> Result<CloudState, String> {
    sync.register(&email, &password, &display_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_logout(sync: tauri::State<'_, Arc<SyncManager>>) -> Result<CloudState, String> {
    sync.logout().await.map_err(|e| e.to_string())?;
    sync.state().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cloud_get_state(sync: tauri::State<'_, Arc<SyncManager>>) -> Result<CloudState, String> {
    sync.state().map_err(|e| e.to_string())
}

/// 当前云端 API 地址（登录界面回显用）。
#[tauri::command]
pub fn cloud_get_api_base(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::cloud::resolve_api_base(&dir))
}

/// 切换云端 API 地址：仅未登录时允许（写 cloud_api_base + 重建 SyncManager）。
#[tauri::command]
pub async fn cloud_set_api_base(
    app: tauri::AppHandle,
    sync: tauri::State<'_, Arc<SyncManager>>,
    api_base: String,
) -> Result<String, String> {
    let base = api_base.trim().trim_end_matches('/').to_string();
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err("地址需以 http:// 或 https:// 开头".into());
    }
    let authenticated = sync.state().map(|s| s.authenticated).unwrap_or(false);
    if authenticated {
        return Err("请先退出登录再切换服务器地址".into());
    }
    // rebuild 内部会 unmanage 当前实例；State 借用旧数据，必须先释放再重建。
    drop(sync);
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::write(dir.join("cloud_api_base"), &base).map_err(|e| e.to_string())?;
    crate::cloud::rebuild(&app, dir)?;
    Ok(base)
}

// ── 笔记本 / 笔记 ────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookInfo {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub sort: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteInfo {
    pub id: String,
    pub relative_path: String,
    pub filename: String,
    pub preview: String,
    pub updated_at: i64,
}

/// 刷新缓存笔记本（拉远端列表 → 建目录 → 启用同步），返回笔记本列表。
#[tauri::command]
pub async fn cloud_refresh_notebooks(
    app: tauri::AppHandle,
) -> Result<Vec<NotebookInfo>, String> {
    let sync = app.state::<Arc<SyncManager>>();
    let state = app.state::<MobileState>();
    cache::ensure_cache_notebooks(&sync, &state).await?;
    notebook_list(app)
}

#[tauri::command]
pub fn notebook_list(app: tauri::AppHandle) -> Result<Vec<NotebookInfo>, String> {
    let state = app.state::<MobileState>();
    Ok(state
        .memo_file
        .read_notebook_configs()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| NotebookInfo {
            id: c.id,
            name: c.name,
            icon: c.icon,
            sort: c.sort,
        })
        .collect())
}

#[tauri::command]
pub fn note_list(app: tauri::AppHandle, notebook_id: String) -> Result<Vec<NoteInfo>, String> {
    let state = app.state::<MobileState>();
    Ok(state
        .memo_file
        .read_all_memos_for_notebook_id(Some(&notebook_id))
        .into_iter()
        .map(|m| NoteInfo {
            id: m.id,
            relative_path: m.relative_path,
            filename: m.filename,
            preview: m.preview,
            updated_at: m.updated_at,
        })
        .collect())
}

#[tauri::command]
pub fn note_read(
    app: tauri::AppHandle,
    notebook_id: String,
    relative_path: String,
) -> Result<String, String> {
    let state = app.state::<MobileState>();
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or("NOTEBOOK_NOT_FOUND")?;
    let path = cache::path_from_relative(std::path::Path::new(&notebook.path), &relative_path)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("READ_NOTE_FAILED: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "CLOUD_NOTE_NOT_UTF8".to_string())
}

/// 保存笔记并记入同步脏队列（Put + 内容指纹）。
#[tauri::command]
pub fn note_save(
    app: tauri::AppHandle,
    notebook_id: String,
    relative_path: String,
    content: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let sync = app.state::<Arc<SyncManager>>();
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or("NOTEBOOK_NOT_FOUND")?;
    // 两段式：排他锁内写文件，释放后注册索引（锁不可重入）
    let path = {
        let _guard = state
            .memo_file
            .operation_locks()
            .notebook_change(&[&notebook_id], "mobile_note_save")
            .map_err(|e| e.to_string())?;
        let path =
            cache::path_from_relative(std::path::Path::new(&notebook.path), &relative_path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        memo_file::atomic_write_bytes(&path, content.as_bytes()).map_err(|e| e.to_string())?;
        path
    };
    state
        .memo_file
        .register_existing_file_for_notebook_id(&notebook_id, &path)
        .map_err(|e| e.to_string())?;
    let note_id = v2_path_note_id(&notebook_id, &relative_path);
    sync.record_v2_local_change(
        &notebook_id,
        &note_id,
        LocalChangeKind::Put,
        &v2_content_hash(content.as_bytes()),
    )
    .map_err(|e| e.to_string())?;
    crate::sync::spawn_debounced_sync(app.clone());
    Ok(())
}

/// 删除笔记：排他锁内删文件，释放后清索引，记入同步脏队列（Delete）。
#[tauri::command]
pub fn note_delete(
    app: tauri::AppHandle,
    notebook_id: String,
    relative_path: String,
) -> Result<(), String> {
    let state = app.state::<MobileState>();
    let sync = app.state::<Arc<SyncManager>>();
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or("NOTEBOOK_NOT_FOUND")?;
    let path = cache::path_from_relative(std::path::Path::new(&notebook.path), &relative_path)?;
    if !path.exists() {
        return Err("NOTE_NOT_FOUND".to_string());
    }
    let note_id = v2_path_note_id(&notebook_id, &relative_path);
    // 两段式：排他锁内删文件，释放后清索引（锁不可重入）
    {
        let _guard = state
            .memo_file
            .operation_locks()
            .notebook_change(&[&notebook_id], "mobile_note_delete")
            .map_err(|e| e.to_string())?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let memo_id = state
        .memo_file
        .find_memo_by_relative_path_for_notebook_id(&notebook_id, &relative_path)
        .map(|memo| memo.id);
    if let Some(memo_id) = memo_id {
        if !state
            .memo_file
            .prune_deleted_memo_for_notebook_id(&notebook_id, &relative_path, &memo_id)
            .map_err(|e| e.to_string())?
        {
            return Err("NOTE_DELETE_CONFLICT: path was recreated".to_string());
        }
    }
    sync.record_v2_local_change(
        &notebook_id,
        &note_id,
        LocalChangeKind::Delete,
        &v2_content_hash(b""),
    )
    .map_err(|e| e.to_string())?;
    crate::sync::spawn_debounced_sync(app.clone());
    Ok(())
}

/// 新建笔记（空 Markdown），返回相对路径。
#[tauri::command]
pub fn note_create(
    app: tauri::AppHandle,
    notebook_id: String,
    title: String,
) -> Result<String, String> {
    let state = app.state::<MobileState>();
    let sync = app.state::<Arc<SyncManager>>();
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or("NOTEBOOK_NOT_FOUND")?;
    let sanitized: String = title
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string();
    let stem = if sanitized.is_empty() {
        "未命名笔记".to_string()
    } else {
        sanitized.chars().take(60).collect()
    };
    let root = std::path::Path::new(&notebook.path);
    let mut candidate = format!("{stem}.md");
    let mut suffix = 1;
    while cache::path_from_relative(root, &candidate)
        .map(|p| p.exists())
        .unwrap_or(true)
    {
        suffix += 1;
        candidate = format!("{stem} ({suffix}).md");
    }
    let path = {
        let _guard = state
            .memo_file
            .operation_locks()
            .notebook_change(&[&notebook_id], "mobile_note_create")
            .map_err(|e| e.to_string())?;
        let path = cache::path_from_relative(root, &candidate)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        memo_file::atomic_create_bytes(&path, b"").map_err(|e| e.to_string())?;
        path
    };
    state
        .memo_file
        .register_existing_file_for_notebook_id(&notebook_id, &path)
        .map_err(|e| e.to_string())?;
    let note_id = v2_path_note_id(&notebook_id, &candidate);
    sync.record_v2_local_change(
        &notebook_id,
        &note_id,
        LocalChangeKind::Put,
        &v2_content_hash(b""),
    )
    .map_err(|e| e.to_string())?;
    crate::sync::spawn_debounced_sync(app.clone());
    Ok(candidate)
}

// ── 附件 ─────────────────────────────────────────────────────────────────

/// 图片 MIME 推断（按扩展名，覆盖常见 web 图片格式）。
fn image_mime(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

/// 安全附件文件名：去路径分隔与非法字符，保留扩展名，超长截断 stem。
fn sanitize_attachment_name(raw: &str) -> String {
    let sanitized: String = raw
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("image")
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string();
    if sanitized.is_empty() {
        return "image".to_string();
    }
    let (stem, extension) = sanitized
        .rsplit_once('.')
        .map(|(stem, ext)| (stem.to_string(), Some(ext.to_string())))
        .unwrap_or((sanitized.clone(), None));
    let stem: String = stem.chars().take(60).collect();
    match extension {
        Some(ext) if !ext.is_empty() => format!("{stem}.{ext}"),
        _ => stem,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSaved {
    /// 笔记本内相对路径（attachments/xxx，同步实体与 markdown 引用基准）
    pub relative_path: String,
    /// 设备本地绝对路径（编辑器 storageKey，桌面同款 asset://localhost 语法）
    pub absolute_path: String,
}

/// 保存图片附件：落盘 attachments/ → 注册媒体索引 → 记入同步脏队列。
/// 同名同内容直接复用；同名不同内容追加内容哈希后缀。
#[tauri::command]
pub fn attachment_save(
    app: tauri::AppHandle,
    notebook_id: String,
    file_name: String,
    data_base64: String,
) -> Result<AttachmentSaved, String> {
    use base64::Engine as _;
    let state = app.state::<MobileState>();
    let sync = app.state::<Arc<SyncManager>>();
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or("NOTEBOOK_NOT_FOUND")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|e| format!("ATTACHMENT_DECODE_FAILED: {e}"))?;
    if bytes.is_empty() {
        return Err("ATTACHMENT_EMPTY".into());
    }
    let name = sanitize_attachment_name(&file_name);
    let root = std::path::Path::new(&notebook.path);
    let hash = v2_content_hash(&bytes);
    let hash_suffix = &hash[..8];
    let mut relative = format!("attachments/{name}");
    let mut target = root.join(&relative);
    if target.exists() {
        if std::fs::read(&target).map(|existing| existing == bytes).unwrap_or(false) {
            return Ok(AttachmentSaved {
                relative_path: relative,
                absolute_path: target.display().to_string(),
            });
        }
        let (stem, extension) = name
            .rsplit_once('.')
            .map(|(stem, ext)| (stem.to_string(), Some(ext.to_string())))
            .unwrap_or((name.clone(), None));
        relative = match extension {
            Some(ext) => format!("attachments/{stem}-{hash_suffix}.{ext}"),
            None => format!("attachments/{stem}-{hash_suffix}"),
        };
        target = root.join(&relative);
    }
    {
        let _guard = state
            .memo_file
            .operation_locks()
            .notebook_change(&[&notebook_id], "mobile_attachment_save")
            .map_err(|e| e.to_string())?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        memo_file::atomic_create_bytes(&target, &bytes).map_err(|e| e.to_string())?;
    }
    // 与桌面 media.rs 同款：ensure_media_resource 不带 policy 检查
    // （refresh_media_resource_path 会把 attachments/ 当被忽略目录拒绝）
    let kind = memo_file::media_kind_for_path(&target)
        .ok_or("ATTACHMENT_TYPE_UNSUPPORTED")?;
    state
        .memo_file
        .ensure_media_resource(&notebook_id, &relative, kind, &target)
        .map_err(|e| e.to_string())?;
    let entity_id = v2_path_note_id(&notebook_id, &relative);
    sync.record_v2_local_change(
        &notebook_id,
        &entity_id,
        LocalChangeKind::Put,
        &v2_content_hash(&bytes),
    )
    .map_err(|e| e.to_string())?;
    crate::sync::spawn_debounced_sync(app.clone());
    Ok(AttachmentSaved {
        relative_path: relative,
        absolute_path: target.display().to_string(),
    })
}

/// 读取附件为 dataURL 供 WebView 渲染。
/// storageKey 兼容三种形态：本设备绝对路径、其他设备绝对路径（按文件名回落）、
/// attachments/ 相对路径——回落到当前笔记本 attachments/ 下同名文件。
#[tauri::command]
pub fn attachment_read(
    app: tauri::AppHandle,
    notebook_id: String,
    storage_key: String,
) -> Result<String, String> {
    use base64::Engine as _;
    let state = app.state::<MobileState>();
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or("NOTEBOOK_NOT_FOUND")?;
    let key = storage_key.trim();
    let candidates = {
        let mut list = Vec::new();
        let direct = std::path::Path::new(key);
        if direct.is_absolute() {
            list.push(direct.to_path_buf());
        }
        let base = direct
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !base.is_empty() && !base.starts_with('.') {
            list.push(std::path::Path::new(&notebook.path).join("attachments").join(&base));
        }
        if !direct.is_absolute() && !key.is_empty() && !key.starts_with('.') {
            if let Ok(path) = cache::path_from_relative(std::path::Path::new(&notebook.path), key) {
                list.push(path);
            }
        }
        list
    };
    for candidate in candidates {
        if let Ok(bytes) = std::fs::read(&candidate) {
            let mime = image_mime(&candidate);
            return Ok(format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ));
        }
    }
    Err("ATTACHMENT_NOT_FOUND".into())
}

// ── 历史版本 ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub revision: String,
    pub deleted: bool,
    pub size_bytes: i64,
    pub sync_seq: i64,
    pub created_at: i64,
}

/// 云端历史版本列表（按 syncSeq 降序，服务端已排好）。
#[tauri::command]
pub async fn note_history(
    app: tauri::AppHandle,
    notebook_id: String,
    relative_path: String,
) -> Result<Vec<HistoryEntry>, String> {
    let sync = app.state::<Arc<SyncManager>>();
    let note_id = v2_path_note_id(&notebook_id, &relative_path);
    let history = sync.v2_history(&note_id).await.map_err(|e| e.to_string())?;
    Ok(history
        .revisions
        .into_iter()
        .filter(|entry| !entry.deleted)
        .map(|entry| HistoryEntry {
            revision: entry.revision,
            deleted: entry.deleted,
            size_bytes: entry.size_bytes,
            sync_seq: entry.sync_seq,
            created_at: entry.created_at,
        })
        .collect())
}

/// 读取指定历史版本正文（含 frontmatter，与磁盘全文同构）。
#[tauri::command]
pub async fn note_history_read(
    app: tauri::AppHandle,
    notebook_id: String,
    relative_path: String,
    revision: String,
) -> Result<String, String> {
    let sync = app.state::<Arc<SyncManager>>();
    let note_id = v2_path_note_id(&notebook_id, &relative_path);
    let (_, bytes) = sync
        .v2_historical_bytes(&note_id, &revision)
        .await
        .map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| "CLOUD_NOTE_NOT_UTF8".to_string())
}

// ── 检查更新 ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub download_url: String,
    pub notes: String,
    pub has_update: bool,
}

/// 查询云端最新版本（GET /v2/app/latest，免认证）。与当前版本不一致即视为有更新。
#[tauri::command]
pub async fn app_check_update(app: tauri::AppHandle) -> Result<UpdateCheck, String> {
    let current_version = app.package_info().version.to_string();
    let base = {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::read_to_string(dir.join("cloud_api_base"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| flowix_sync::DEFAULT_CLOUD_API_BASE.to_string())
    };
    let response: serde_json::Value = reqwest::get(format!("{base}/v2/app/latest"))
        .await
        .map_err(|e| format!("UPDATE_CHECK_FAILED: {e}"))?
        .json()
        .await
        .map_err(|e| format!("UPDATE_CHECK_PARSE_FAILED: {e}"))?;
    let data = response.get("data").cloned().unwrap_or(response);
    let latest_version = data
        .get("versionName")
        .and_then(|v| v.as_str())
        .unwrap_or(&current_version)
        .to_string();
    let download_url = data
        .get("downloadUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let notes = data
        .get("notes")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let has_update = !latest_version.is_empty()
        && !download_url.is_empty()
        && latest_version != current_version;
    Ok(UpdateCheck {
        current_version,
        latest_version,
        download_url,
        notes,
        has_update,
    })
}

/// 用系统浏览器打开下载页（opener → Android CustomTabs/浏览器）。
#[tauri::command]
pub async fn app_open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

// ── 搜索 ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub notebook_id: String,
    pub relative_path: String,
    pub title: String,
    pub snippet: String,
}

/// 跨全部缓存笔记本的全文搜索（每本上限 20 条）。
#[tauri::command]
pub fn search(app: tauri::AppHandle, query: String) -> Result<Vec<SearchHit>, String> {
    let state = app.state::<MobileState>();
    let mut hits = Vec::new();
    for config in state
        .memo_file
        .read_notebook_configs()
        .map_err(|e| e.to_string())?
    {
        for hit in state
            .memo_file
            .search_notes(&config.id, &query, 20)
            .map_err(|e| e.to_string())?
        {
            hits.push(SearchHit {
                notebook_id: config.id.clone(),
                relative_path: hit.relative_path,
                title: hit.title,
                snippet: hit.snippet,
            });
        }
    }
    Ok(hits)
}

// ── 同步 ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn cloud_sync_now(app: tauri::AppHandle) -> Result<SyncSummary, String> {
    sync::sync_now(&app).await
}

// ── M0.3 spike 保留 ──────────────────────────────────────────────────────

#[tauri::command]
pub fn init_cache(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}
