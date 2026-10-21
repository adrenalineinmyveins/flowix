//! 云端笔记本的本地缓存适配层（安卓版 local_adapter）。
//!
//! 镜像桌面端 commands/cloud/local_adapter 的职责，M1 简化决策：
//! - 无文件 watcher：写入只来自本 App，省去 self-write 标记与 stage 文件；
//!   回声抑制（P0-2）与本地领先保护（P1-3）两条安全红线完整保留。
//! - 无桌面 memo_events 广播：改为向 WebView 发 `cache-notes-changed`。
//! - 附件/删除/移动类冲突在 M1 直接报错保留现场，Markdown put 冲突走
//!   三方合并（见 sync.rs）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use flowix_core::memo_file::{self, MemoFile};
use flowix_sync::{
    v2_content_hash, v2_local_content_diverged, v2_path_note_id, SyncManager,
    V2AccountSyncReport, V2LocalNote, V2LocalNotebook, V2RemoteApply,
};
use tauri::{Emitter, Manager};

pub const CACHE_NOTES_CHANGED_EVENT: &str = "cache-notes-changed";

pub struct MobileState {
    pub memo_file: MemoFile,
    pub cache_root: PathBuf,
}

/// 拉取远端笔记本列表，为每个笔记本建立缓存目录并启用同步。幂等。
pub async fn ensure_cache_notebooks(
    sync: &SyncManager,
    state: &MobileState,
) -> Result<usize, String> {
    let remote = sync
        .v2_remote_notebooks()
        .await
        .map_err(|e| e.to_string())?;
    let mut configs = state
        .memo_file
        .read_notebook_configs()
        .map_err(|e| e.to_string())?;
    let known: HashSet<String> = configs.iter().map(|c| c.id.clone()).collect();
    let now = chrono::Utc::now().timestamp_millis();
    let mut created = 0;
    for notebook in &remote {
        if known.contains(&notebook.id) {
            continue;
        }
        let path = state.cache_root.join(&notebook.id);
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        configs.push(memo_file::NotebookConfig {
            id: notebook.id.clone(),
            name: notebook.name.clone(),
            icon: notebook.icon.clone(),
            path: path.to_string_lossy().into_owned(),
            is_default: false,
            sort: notebook.sort_order,
            created_at: now,
            updated_at: now,
        });
        created += 1;
    }
    if created > 0 {
        state
            .memo_file
            .write_notebook_configs(&configs)
            .map_err(|e| e.to_string())?;
    }
    let enabled: HashSet<String> = sync
        .v2_enabled_notebooks()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|n| n.notebook_id)
        .collect();
    for notebook in &remote {
        if enabled.contains(&notebook.id) {
            continue;
        }
        sync.set_v2_notebook_enabled(
            &V2LocalNotebook {
                id: notebook.id.clone(),
                name: notebook.name.clone(),
                icon: notebook.icon.clone(),
                sort_order: notebook.sort_order,
            },
            true,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(created)
}

/// 相对路径 → 绝对路径（拒绝 `..`、绝对路径、反斜杠、隐藏段）。
pub fn path_from_relative(base: &Path, relative: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(relative);
    if relative.contains('\\')
        || relative.starts_with('/')
        || candidate
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || candidate
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Err(format!("INVALID_RELATIVE_PATH: {relative}"));
    }
    let mut path = base.to_path_buf();
    for component in candidate.components() {
        path.push(component.as_os_str());
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() {
                return Err(format!("CLOUD_PATH_SYMLINK: {relative}"));
            }
        }
    }
    Ok(path)
}

fn scan_attachments(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            scan_attachments(root, &path, output)?;
        } else if metadata.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

/// 缓存笔记本 → 同步引擎快照。M1 采用全量快照 + 索引对账：
/// 缓存笔记本规模可控，全量最稳，删除检测也依赖它。
pub fn snapshot(
    sync: &SyncManager,
    state: &MobileState,
) -> Result<(Vec<V2LocalNotebook>, Vec<V2LocalNote>), String> {
    let enabled: HashSet<String> = sync
        .v2_enabled_notebooks()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|n| n.notebook_id)
        .collect();
    let configs = state
        .memo_file
        .read_notebook_configs()
        .map_err(|e| e.to_string())?;
    let mut notebooks = Vec::new();
    let mut notes = Vec::new();
    for config in configs.iter().filter(|c| enabled.contains(&c.id)) {
        let root = Path::new(&config.path);
        if !root.is_dir() {
            return Err(format!("CLOUD_NOTEBOOK_UNAVAILABLE: {}", root.display()));
        }
        state
            .memo_file
            .reconcile_notebook_with_disk_bidirectional(&config.id)
            .map_err(|e| e.to_string())?;
        for memo in state
            .memo_file
            .read_all_memos_for_notebook_id(Some(&config.id))
        {
            let path = path_from_relative(root, &memo.relative_path)?;
            let content = std::fs::read(&path)
                .map_err(|e| format!("READ_NOTE_FAILED {}: {e}", path.display()))?;
            notes.push(V2LocalNote {
                id: v2_path_note_id(&config.id, &memo.relative_path),
                notebook_id: config.id.clone(),
                filename: memo.relative_path,
                content,
                attachments: Vec::new(),
            });
        }
        let mut attachment_paths = Vec::new();
        scan_attachments(root, &root.join("attachments"), &mut attachment_paths)?;
        for path in attachment_paths {
            let relative = memo_file::notebook_relative_path(root, &path)?;
            notes.push(V2LocalNote {
                id: v2_path_note_id(&config.id, &relative),
                notebook_id: config.id.clone(),
                content: std::fs::read(&path).map_err(|e| e.to_string())?,
                filename: relative,
                attachments: Vec::new(),
            });
        }
        notebooks.push(V2LocalNotebook {
            id: config.id.clone(),
            name: config.name.clone(),
            icon: config.icon.clone(),
            sort_order: config.sort,
        });
    }
    Ok((notebooks, notes))
}

fn write_attachments(
    base: &Path,
    attachments: &[flowix_sync::V2RemoteAttachment],
) -> Result<(), String> {
    for attachment in attachments {
        let filename = &attachment.metadata.filename;
        if attachment.metadata.size_bytes != i64::try_from(attachment.content.len()).unwrap_or(-1)
            || v2_content_hash(&attachment.content) != attachment.metadata.content_hash
        {
            return Err(format!("CLOUD_ATTACHMENT_INVALID: {filename}"));
        }
        let relative = if filename.starts_with("attachments/") {
            filename.clone()
        } else {
            format!("attachments/{filename}")
        };
        let path = path_from_relative(base, &relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        match memo_file::atomic_create_bytes(&path, &attachment.content) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if std::fs::read(&path).map_err(|e| e.to_string())? != attachment.content {
                    return Err(format!("CLOUD_ATTACHMENT_EDIT_CONFLICT: {filename}"));
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// 远端变更 → 缓存落盘（先 apply 成功，引擎才提交 cursor，顺序由
/// complete_v2_sync_with_apply 保证）。
pub fn apply_v2_report(
    app: &tauri::AppHandle,
    state: &MobileState,
    sync: &SyncManager,
    report: &V2AccountSyncReport,
) -> Result<(), String> {
    let mut note_changes: HashMap<String, Vec<&V2RemoteApply>> = HashMap::new();
    let mut notebook_metadata: HashMap<String, (Option<String>, Option<String>, Option<i64>, bool)> =
        HashMap::new();
    let mut thread_changes: Vec<(&str, bool, Option<&[u8]>)> = Vec::new();
    for change in &report.remote {
        match change {
            V2RemoteApply::Notebook {
                notebook_id,
                name,
                icon,
                sort_order,
                deleted,
                ..
            } => {
                notebook_metadata.insert(
                    notebook_id.clone(),
                    (name.clone(), icon.clone(), *sort_order, *deleted),
                );
            }
            V2RemoteApply::Note { notebook_id, .. } => {
                note_changes.entry(notebook_id.clone()).or_default().push(change);
            }
            // A4：远端线程幂等并集落库（另一端的消息/新会话/删除到达）
            V2RemoteApply::Thread {
                thread_id,
                deleted,
                content,
                ..
            } => {
                thread_changes.push((thread_id, *deleted, content.as_deref()));
            }
        }
    }

    let mut threads_changed = false;
    if !thread_changes.is_empty() {
        let agent = app.state::<std::sync::Arc<crate::agent::AgentState>>();
        for (thread_id, deleted, content) in thread_changes {
            if crate::agent::apply_remote_thread(&agent, thread_id, deleted, content)? {
                threads_changed = true;
            }
        }
    }
    if threads_changed {
        let _ = tauri::Emitter::emit(app, crate::agent::THREADS_CHANGED_EVENT, ());
    }

    let mut changed = threads_changed;
    for (notebook_id, changes) in note_changes {
        // 两段式（锁不可重入）：排他锁内只做文件操作，锁释放后再做索引
        // 注册/清理（register/prune 内部会取共享锁，与排他锁自锁）。
        enum DeferredIndex {
            Register { path: PathBuf },
            Prune { filename: String, memo_id: String },
        }
        let mut deferred: Vec<DeferredIndex> = Vec::new();
        {
            let _guard = state
                .memo_file
                .operation_locks()
                .notebook_change(&[&notebook_id], "mobile_apply_cloud")
                .map_err(|e| e.to_string())?;
        let notebook = state
            .memo_file
            .get_notebook_config_by_id(&notebook_id)
            .ok_or_else(|| "NOTEBOOK_NOT_FOUND".to_string())?;
        let base = PathBuf::from(&notebook.path);
        for change in changes {
            let V2RemoteApply::Note {
                note_id,
                filename,
                content_hash,
                content,
                deleted,
                attachments,
                ..
            } = change
            else {
                continue;
            };
            if filename.starts_with("attachments/") {
                let path = path_from_relative(&base, filename)?;
                if *deleted {
                    if path.exists() {
                        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    }
                    continue;
                }
                let bytes = content
                    .as_ref()
                    .ok_or_else(|| format!("CLOUD_ATTACHMENT_CONTENT_MISSING: {filename}"))?;
                let expected = content_hash
                    .as_deref()
                    .ok_or_else(|| format!("CLOUD_ATTACHMENT_HASH_MISSING: {filename}"))?;
                if v2_content_hash(bytes) != expected {
                    return Err(format!("CLOUD_ATTACHMENT_HASH_MISMATCH: {filename}"));
                }
                let local = std::fs::read(&path).ok();
                if local.as_deref() == Some(bytes.as_slice()) {
                    continue;
                }
                if local.is_some() {
                    // 附件被本地改过且未在脏队列 → M1 保留现场报错
                    return Err(format!("CLOUD_ATTACHMENT_EDIT_CONFLICT: {filename}"));
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                memo_file::atomic_create_bytes(&path, bytes).map_err(|e| e.to_string())?;
                continue;
            }
            if *deleted {
                if let Some(memo) = state
                    .memo_file
                    .find_memo_by_relative_path_for_notebook_id(&notebook_id, filename)
                {
                    let path = path_from_relative(&base, &memo.relative_path)?;
                    let local = std::fs::read(&path).ok();
                    let local_hash = local.as_ref().map(|b| v2_content_hash(b));
                    let baseline = sync
                        .v2_note_state(note_id)
                        .ok()
                        .flatten()
                        .and_then(|s| s.content_hash);
                    let pending = sync.has_pending_v2_note_change(note_id).unwrap_or(false);
                    if v2_local_content_diverged(
                        local_hash.as_deref(),
                        baseline.as_deref(),
                        pending,
                    ) {
                        return Err(format!("CLOUD_LOCAL_DELETE_CONFLICT: {filename}"));
                    }
                    if path.exists() {
                        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    }
                    deferred.push(DeferredIndex::Prune {
                        filename: filename.clone(),
                        memo_id: memo.id.clone(),
                    });
                    changed = true;
                }
                continue;
            }
            let bytes = content
                .as_ref()
                .ok_or_else(|| format!("CLOUD_NOTE_CONTENT_MISSING: {note_id}"))?;
            let expected = content_hash
                .as_deref()
                .ok_or_else(|| format!("CLOUD_NOTE_HASH_MISSING: {note_id}"))?;
            if v2_content_hash(bytes) != expected {
                return Err(format!("CLOUD_NOTE_HASH_MISMATCH: {note_id}"));
            }
            std::str::from_utf8(bytes)
                .map_err(|_| format!("CLOUD_NOTE_NOT_UTF8: {note_id}"))?;
            write_attachments(&base, attachments)?;
            let desired = path_from_relative(&base, filename)?;
            if !desired.extension().is_some_and(|e| e == "md") {
                return Err(format!("INVALID_CLOUD_FILENAME: {filename}"));
            }
            let disk = std::fs::read(&desired).ok();
            let disk_hash = disk.as_ref().map(|b| v2_content_hash(b));
            // P0-2 回声抑制：内容一致则跳过写盘
            if disk_hash.as_deref() == Some(expected) {
                if state
                    .memo_file
                    .find_memo_by_relative_path_for_notebook_id(&notebook_id, filename)
                    .is_none()
                {
                    deferred.push(DeferredIndex::Register {
                        path: desired.clone(),
                    });
                }
                continue;
            }
            // P1-3 本地领先保护：磁盘偏离基线或有未推送编辑 → 不覆盖
            let baseline = sync
                .v2_note_state(note_id)
                .ok()
                .flatten()
                .and_then(|s| s.content_hash);
            let pending = sync.has_pending_v2_note_change(note_id).unwrap_or(false);
            if v2_local_content_diverged(disk_hash.as_deref(), baseline.as_deref(), pending) {
                return Err(format!("CLOUD_LOCAL_EDIT_CONFLICT: {filename}"));
            }
            if let Some(parent) = desired.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            memo_file::atomic_write_bytes(&desired, bytes).map_err(|e| e.to_string())?;
            deferred.push(DeferredIndex::Register {
                path: desired.clone(),
            });
            changed = true;
        }
        }
        // 排他锁已释放，安全执行索引注册/清理
        for action in deferred {
            match action {
                DeferredIndex::Register { path } => {
                    state
                        .memo_file
                        .register_existing_file_for_notebook_id(&notebook_id, &path)
                        .map_err(|e| e.to_string())?;
                }
                DeferredIndex::Prune { filename, memo_id } => {
                    state
                        .memo_file
                        .prune_deleted_memo_for_notebook_id(&notebook_id, &filename, &memo_id)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }

    if !notebook_metadata.is_empty() {
        let mut configs = state
            .memo_file
            .read_notebook_configs()
            .map_err(|e| e.to_string())?;
        configs.retain(|config| {
            !notebook_metadata
                .get(&config.id)
                .is_some_and(|(_, _, _, deleted)| *deleted)
        });
        for config in &mut configs {
            if let Some((name, icon, sort_order, deleted)) = notebook_metadata.get(&config.id) {
                if *deleted {
                    continue;
                }
                if let Some(name) = name {
                    config.name.clone_from(name);
                }
                config.icon.clone_from(icon);
                if let Some(sort) = sort_order {
                    config.sort = *sort;
                }
            }
        }
        state
            .memo_file
            .write_notebook_configs(&configs)
            .map_err(|e| e.to_string())?;
        changed = true;
    }

    if changed {
        let _ = app.emit(CACHE_NOTES_CHANGED_EVENT, ());
    }
    Ok(())
}
