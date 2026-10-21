//! 方案 A3 桌面桥：thread.db ↔ 云同步引擎的粘合层。
//!
//! 写路径采用「pass 前对账」而非写入点插桩——流式期间每个 chunk 都落库，
//! 逐点 hook 无法防抖；改为每次账户级同步 pass 组装快照前：
//! 1. 按 `threads.updated_at` 增量选出活跃线程（内存水位，重启首轮全量）；
//! 2. 投影 payload 计算 `v2_content_hash`，与 `v2_thread_states` 云端基线
//!    比对——相同则 clean（apply 回写天然防回环），不同则
//!    `record_v2_thread_change(Put)`（fingerprint 幂等，同 hash 不涨 generation）；
//! 3. 云端有基线而本地无消息的线程 → 本地已删 → `Delete` 入队。
//!
//! 兜底调度：30s 账户 Poll 与笔记本活动触发的 pass 都会跑对账，消息
//! 最迟 30s 上云（与 A4 验收窗口一致）。

use crate::app::state::AppState;
use flowix_sync::{LocalChangeKind, V2LocalThread, V2ThreadPayload};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// 进程级水位：每线程上次对账见到的 `threads.updated_at`。
/// 重启丢失 → 首轮全量对账（一次性开销，线程数量级小）。
fn last_seen_updated_at() -> &'static Mutex<HashMap<String, i64>> {
    static LAST_SEEN: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    LAST_SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// pass 前对账：把本地线程变更翻译成 sync 引擎的 dirty 队列。
/// 未登录时直接跳过（不扫不推水位，避免登录后增量漏扫）。
pub(crate) async fn reconcile_thread_dirty_state(state: &AppState) -> Result<(), String> {
    if !state
        .cloud_sync
        .has_v2_account()
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    let watermark = {
        let seen = last_seen_updated_at()
            .lock()
            .map_err(|_| "thread bridge watermark poisoned".to_string())?;
        seen.values().copied().min().unwrap_or(0)
    };
    let active = state
        .thread_manager
        .thread_ids_with_messages_since(watermark)
        .await
        .map_err(|e| e.to_string())?;
    let mut refreshed = Vec::with_capacity(active.len());
    for (local_thread_id, updated_at) in active {
        let cloud_id = crate::agent_session::ThreadManager::cloud_thread_id(&local_thread_id);
        let Some(payload) = state
            .thread_manager
            .thread_payload_for_cloud(&local_thread_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        // clean 判定：本地投影哈希 == 云端基线 blob 哈希 → 无须重推
        //（apply 回写后天然 clean，防回环震荡）。
        let fingerprint = serde_json::to_vec(&payload)
            .map(|bytes| flowix_sync::v2_content_hash(&bytes))
            .map_err(|e| e.to_string())?;
        let clean = state
            .cloud_sync
            .v2_thread_state(&cloud_id)
            .map_err(|e| e.to_string())?
            .is_some_and(|base| base.content_hash.as_deref() == Some(fingerprint.as_str()));
        if !clean {
            state
                .cloud_sync
                .record_v2_thread_change(&cloud_id, LocalChangeKind::Put, &fingerprint)
                .map_err(|e| e.to_string())?;
        }
        refreshed.push((local_thread_id, updated_at));
    }
    {
        let mut seen = last_seen_updated_at()
            .lock()
            .map_err(|_| "thread bridge watermark poisoned".to_string())?;
        seen.extend(refreshed);
    }

    // 删除对账：云端基线存在（未删）而本地已无消息 → 本地删除传播上云。
    let local_ids: HashSet<String> = state
        .thread_manager
        .all_thread_ids_with_messages()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    for cloud_state in state
        .cloud_sync
        .v2_all_thread_states()
        .map_err(|e| e.to_string())?
    {
        if cloud_state.deleted {
            continue;
        }
        let Some(local_id) =
            crate::agent_session::ThreadManager::local_thread_id(&cloud_state.thread_id)
        else {
            continue;
        };
        if !local_ids.contains(local_id) {
            state
                .cloud_sync
                .record_v2_thread_change(&cloud_state.thread_id, LocalChangeKind::Delete, "deleted")
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 快照注入：dirty thread → `V2LocalThread`（引擎 freeze 据此上传 blob）。
pub(crate) async fn v2_thread_snapshot(state: &AppState) -> Result<Vec<V2LocalThread>, String> {
    let dirty_ids = state
        .cloud_sync
        .v2_dirty_thread_ids()
        .map_err(|e| e.to_string())?;
    let mut threads = Vec::with_capacity(dirty_ids.len());
    for cloud_id in dirty_ids {
        let Some(local_id) = crate::agent_session::ThreadManager::local_thread_id(&cloud_id)
        else {
            continue;
        };
        let Some(payload) = state
            .thread_manager
            .thread_payload_for_cloud(local_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            // 本地线程消失（删除对账下轮会转 Delete）：跳过，不产出操作
            continue;
        };
        threads.push(V2LocalThread {
            id: cloud_id,
            payload,
        });
    }
    Ok(threads)
}

/// 读路径 apply：远端（或 409 合并产物）线程落库并广播 UI 事件。
/// 返回是否有变化（新线程/新消息/删除），供聚合 emit。
pub(crate) async fn apply_remote_thread(
    state: &AppState,
    thread_id: &str,
    deleted: bool,
    content: Option<&[u8]>,
) -> Result<bool, String> {
    if deleted {
        let Some(local_id) = crate::agent_session::ThreadManager::local_thread_id(thread_id)
        else {
            return Ok(false);
        };
        let removed = state
            .thread_manager
            .delete_thread_with_agent_conversations(local_id)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(removed);
    }
    let Some(bytes) = content else {
        return Ok(false);
    };
    let payload: V2ThreadPayload =
        serde_json::from_slice(bytes).map_err(|e| format!("parse cloud thread payload: {e}"))?;
    state
        .thread_manager
        .upsert_thread_from_cloud(&payload)
        .await
        .map_err(|e| e.to_string())
}
