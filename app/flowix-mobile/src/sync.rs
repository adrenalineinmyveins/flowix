//! 同步回路：快照 → 引擎 → 冲突消解 → 应用落盘 → bootstrap 分页推进。
//! 镜像桌面端 coordinator 的单趟流程，去掉防抖/轮询调度（M1 手动触发 +
//! 命令内自动触发，后台调度器 M1 末期再补）。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use flowix_sync::{
    v2_content_hash, v2_path_note_id, LocalChangeKind, SyncError, SyncManager,
    V2AccountSyncReport, V2ConflictMaterial,
};
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::cache::{self, MobileState};

const SYNC_PASS_TIMEOUT: Duration = Duration::from_secs(180);
const BOOTSTRAP_PAGE_LIMIT: usize = 10_000;

/// 串行化并发 sync_now（自动轮询 / 手动 / 保存防抖可能叠加）。
static SYNC_NOW_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 启动后自动同步循环：5 秒首同步（等会话恢复），之后 30 秒轮询。
/// 仅在已认证时执行；错误吞掉等下一轮。
pub fn spawn_auto_sync(app: tauri::AppHandle, sync: Arc<SyncManager>) {
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let delay = if first { Duration::from_secs(5) } else { Duration::from_secs(30) };
            first = false;
            tokio::time::sleep(delay).await;
            let authenticated = sync.state().map(|s| s.authenticated).unwrap_or(false);
            if authenticated {
                let _ = sync_now(&app).await;
            }
        }
    });
}

/// 保存/新建后的防抖推送：3 秒后触发一次同步。
pub fn spawn_debounced_sync(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let _ = sync_now(&app).await;
    });
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub notebooks: usize,
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted: usize,
}

impl SyncSummary {
    fn absorb(&mut self, report: &V2AccountSyncReport) {
        self.uploaded += report.uploaded;
        self.deleted += report.deleted;
        self.downloaded += report.remote.len();
    }
}

/// 手动/自动同步入口：确保缓存笔记本 → 循环同步直至追上远端 head。
pub async fn sync_now(app: &tauri::AppHandle) -> Result<SyncSummary, String> {
    let _serial = SYNC_NOW_LOCK.lock().await;
    let sync = app.state::<Arc<SyncManager>>();
    let state = app.state::<MobileState>();
    cache::ensure_cache_notebooks(&sync, &state).await?;
    let enabled = sync.v2_enabled_notebooks().map_err(|e| e.to_string())?;
    let generation = sync.session_generation();

    let mut summary = SyncSummary::default();
    let mut first_head: Option<i64> = None;
    let mut previous_cursor: Option<i64> = None;
    loop {
        let mut report = sync_pass_once(app, &sync, &state, generation).await?;
        summary.absorb(&report);
        let mut pages = 0;
        while report.bootstrap_next_page_token.is_some() {
            pages += 1;
            if pages > BOOTSTRAP_PAGE_LIMIT {
                return Err("CLOUD_BOOTSTRAP_PAGE_LIMIT".into());
            }
            let next = tokio::time::timeout(
                SYNC_PASS_TIMEOUT,
                sync.v2_continue_bootstrap(&report, None),
            )
            .await
            .map_err(|_| "CLOUD_BOOTSTRAP_TIMEOUT".to_string())?
            .map_err(|e| e.to_string())?;
            sync.complete_v2_sync_with_apply(&next, None, || {
                cache::apply_v2_report(app, &state, &sync, &next)
                    .map_err(SyncError::InvalidState)
            })
            .map_err(|e| e.to_string())?;
            summary.absorb(&next);
            report = next;
        }
        let head = *first_head.get_or_insert(report.head_cursor);
        if report.cursor >= head {
            break;
        }
        if previous_cursor.is_some_and(|cursor| report.cursor <= cursor) {
            return Err("CLOUD_SYNC_STALLED".into());
        }
        previous_cursor = Some(report.cursor);
    }
    summary.notebooks = enabled.len();
    Ok(summary)
}

async fn sync_pass_once(
    app: &tauri::AppHandle,
    sync: &Arc<SyncManager>,
    state: &MobileState,
    generation: u64,
) -> Result<V2AccountSyncReport, String> {
    let (mut notebooks, mut notes) = cache::snapshot(sync, state)?;
    // 全量快照的删除检测：本地已不存在的笔记补记 Delete（对齐桌面端）
    {
        let present: HashSet<&str> = notes.iter().map(|n| n.id.as_str()).collect();
        let enabled_ids: HashSet<String> = sync
            .v2_enabled_notebooks()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|n| n.notebook_id)
            .collect();
        for previous in sync.v2_all_note_states().map_err(|e| e.to_string())? {
            if !previous.deleted
                && enabled_ids.contains(&previous.notebook_id)
                && !present.contains(previous.note_id.as_str())
            {
                sync.record_v2_local_change(
                    &previous.notebook_id,
                    &previous.note_id,
                    LocalChangeKind::Delete,
                    "deleted",
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    // 内容对账（桌面 reconcile_v2_snapshot 的移动版）：本地文件哈希与云端
    // 基线不一致 → 补记 Put。覆盖 login-bootstrap 轮之后的遗留本地编辑
    //（logout 清 states、修复 A 让首轮纯拉取，真实的离线编辑在此补推，
    // 否则会静默分叉直到下次手动编辑）。fingerprint 幂等：与云端一致则零推。
    for note in &notes {
        let Some(baseline) = sync.v2_note_state(&note.id).map_err(|e| e.to_string())? else {
            continue;
        };
        if baseline.deleted {
            continue;
        }
        let local_hash = v2_content_hash(&note.content);
        if baseline.content_hash.as_deref() != Some(local_hash.as_str()) {
            sync.record_v2_local_change(
                &note.notebook_id,
                &note.id,
                LocalChangeKind::Put,
                &local_hash,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    // A4：本地线程快照注入（dirty thread → V2LocalThread，引擎 freeze 上传）
    let agent_state = app.state::<std::sync::Arc<crate::agent::AgentState>>();
    let threads = crate::agent::local_thread_snapshot(&agent_state, sync)?;
    let mut conflict_attempts = 0;
    let report = loop {
        let result = tokio::time::timeout(
            SYNC_PASS_TIMEOUT,
            sync.sync_v2_snapshot_at_generation(
                None,
                notebooks.clone(),
                notes.clone(),
                threads.clone(),
                generation,
            ),
        )
        .await
        .map_err(|_| "云同步超时，请检查网络后重试。".to_string())?;
        match result {
            Err(SyncError::MoveConflict { .. }) => {
                // 移动冲突 M1 不支持自动消解（移动端无重命名 UI，出现概率低）
                return Err("CLOUD_MOVE_CONFLICT_NOT_SUPPORTED_M1".into());
            }
            Err(SyncError::RevisionConflict {
                notebook_id,
                note_id,
                operation_id,
                operation_kind,
                ..
            }) if operation_kind == "put" && conflict_attempts < 3 => {
                conflict_attempts += 1;
                let material = sync
                    .v2_conflict_material(&notebook_id, &note_id, &operation_id)
                    .await
                    .map_err(|e| e.to_string())?;
                resolve_put_conflict(app, sync, state, &material)?;
                (notebooks, notes) = cache::snapshot(sync, state)?;
            }
            other => break other.map_err(|e| e.to_string())?,
        }
    };
    sync.complete_v2_sync_with_apply(&report, None, || {
        cache::apply_v2_report(app, state, sync, &report).map_err(SyncError::InvalidState)
    })
    .map_err(|e| e.to_string())?;
    Ok(report)
}

/// Markdown put 冲突消解：与桌面 choose_markdown_conflict_content 一致——
/// base 存在则三方合并，合并失败或无 base 时采用远端、本地存冲突副本。
/// 远端已删除时对齐桌面语义：接受云端删除（删本地文件 + 索引清理），
/// 不再以 M1 不支持卡死整个 pass（副本风暴现场的 loop-note-2 即此）。
fn resolve_put_conflict(
    app: &tauri::AppHandle,
    sync: &Arc<SyncManager>,
    state: &MobileState,
    material: &V2ConflictMaterial,
) -> Result<(), String> {
    if material.remote.filename.starts_with("attachments/") || material.relocated.is_some() {
        return Err("CLOUD_CONFLICT_KIND_NOT_SUPPORTED_M1".into());
    }
    let notebook = state
        .memo_file
        .get_notebook_config_by_id(&material.notebook_id)
        .ok_or_else(|| "NOTEBOOK_NOT_FOUND".to_string())?;
    // 两段式：排他锁内做文件操作，释放后再注册索引（锁不可重入）
    let (path, next) = {
        let _guard = state
            .memo_file
            .operation_locks()
            .notebook_change(&[&material.notebook_id], "mobile_resolve_conflict")
            .map_err(|e| e.to_string())?;
        let base = Path::new(&notebook.path);
        let path = cache::path_from_relative(base, &material.remote.filename)?;
        if material.remote.deleted {
            // 接受云端删除：本地文件存在则移除（保留逻辑与索引 prune 对齐
            // apply 的 deleted 分支），冲突操作按远端头结算。
            if std::fs::remove_file(&path).is_ok() {
                state
                    .memo_file
                    .prune_deleted_memo_for_notebook_id(
                        &material.notebook_id,
                        &material.remote.filename,
                        &v2_path_note_id(&material.notebook_id, &material.remote.filename),
                    )
                    .map_err(|e| e.to_string())?;
                let _ = app.emit("cache-notes-changed", ());
            }
            sync.v2_rebase_after_conflict(&material.remote)
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let local = std::fs::read(&path).map_err(|e| format!("CONFLICT_LOCAL_READ_FAILED: {e}"))?;
        let remote = material
            .remote_content
            .as_deref()
            .ok_or("CLOUD_CONFLICT_REMOTE_UNAVAILABLE")?;
        let (next, archive_local) = match material.base_content.as_deref() {
            Some(base_bytes) => {
                match flowix_sync::text_merge::merge_markdown(base_bytes, &local, remote) {
                    flowix_sync::text_merge::MergeOutcome::Merged(bytes) => (bytes, false),
                    _ => (remote.to_vec(), true),
                }
            }
            None => (remote.to_vec(), true),
        };
        if archive_local {
            let stem = path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("Note")
                .chars()
                .take(80)
                .collect::<String>();
            let copy = path.with_file_name(format!("{stem} (Flowix conflict {}).md", material.operation_id));
            let _ = flowix_core::memo_file::atomic_create_bytes(&copy, &local);
        }
        flowix_core::memo_file::atomic_write_bytes(&path, &next).map_err(|e| e.to_string())?;
        (path, next)
    };
    state
        .memo_file
        .register_existing_file_for_notebook_id(&material.notebook_id, &path)
        .map_err(|e| e.to_string())?;
    let note_id = v2_path_note_id(&material.notebook_id, &material.remote.filename);
    sync.record_v2_local_change(
        &material.notebook_id,
        &note_id,
        LocalChangeKind::Put,
        &v2_content_hash(&next),
    )
    .map_err(|e| e.to_string())?;
    sync.v2_rebase_after_conflict(&material.remote)
        .map_err(|e| e.to_string())
}
