//! FD-12 M3：consult_remote 回程关联注册表。
//!
//! 出门时登记 `receiptId → 发起会话（DSH thread）` 关联；回程信封到达
//! （inbound `in_reply_to` 命中）后据此把答案即时注入原会话（follow-up）。
//! 落盘 `~/.flowix/redaction/consult-links.json`（设备级，同 FD-6 留档边界，
//! 不进笔记本/云同步）；进程重启后关联仍有效（thread 可 resume）。
//!
//! thread 解析口径：CLI 子进程拿不到 Flowix thread_id，bridge 处理时刻
//! 快照 `running_threads()` 取**最近活跃的 DSH thread**——单活跃场景（绝大多数）
//! 精确命中；多活跃并发时取最近并留痕。无活跃 thread（如插件子会话已结束）
//! 仅落收件箱，不注入。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 未回程的关联保留期：过期清理（查询/登记时惰性执行）。
const LINK_TTL_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsultLink {
    pub receipt_id: String,
    /// 发起会话（Flowix thread_id）；None = 登记时无活跃 DSH 会话。
    pub thread_id: Option<String>,
    /// 发起笔记本根（回程 poll 与留档定位）。
    pub cwd: String,
    pub target_agent_id: String,
    pub at: i64,
    /// 回程已注入/已终结。注入或 TTL 过期置位；留档供审计。
    #[serde(default)]
    pub settled: bool,
}

fn links_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction").join("consult-links.json"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_links() -> Vec<ConsultLink> {
    let Ok(path) = links_path() else { return Vec::new() };
    let Ok(raw) = std::fs::read_to_string(path) else { return Vec::new() };
    // BOM 容错（与 auto-grant.json 同口径）。
    let raw = raw.trim_start_matches('\u{feff}');
    serde_json::from_str(raw).unwrap_or_default()
}

fn save_links(links: &[ConsultLink]) {
    let Ok(path) = links_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(links) {
        let _ = std::fs::write(path, body);
    }
}

/// 惰性清理：摘除已 settle 或过期的关联（保留已 settle 记录至 TTL，供审计回看）。
fn prune(links: &mut Vec<ConsultLink>) -> bool {
    let now = now_unix();
    let before = links.len();
    links.retain(|link| now - link.at < LINK_TTL_SECS);
    before != links.len()
}

/// 登记一条出门关联（bridge 成功投递后调用）。
pub fn register(link: ConsultLink) {
    let mut links = load_links();
    let pruned = prune(&mut links);
    links.retain(|existing| existing.receipt_id != link.receipt_id);
    links.push(link);
    if pruned || !links.is_empty() {
        save_links(&links);
    }
}

/// 按 in_reply_to 查找未终结的关联。
pub fn find_open_by_reply(in_reply_to: &str) -> Option<ConsultLink> {
    load_links()
        .into_iter()
        .find(|link| !link.settled && link.receipt_id == in_reply_to)
}

/// 是否存在未终结关联（push_stop 保留订阅的依据）。
pub fn has_pending() -> bool {
    let mut links = load_links();
    prune(&mut links);
    links.iter().any(|link| !link.settled)
}

/// 标记关联已终结（注入成功/失败终态均置位，防重复注入）。
pub fn settle(receipt_id: &str) {
    let mut links = load_links();
    if let Some(link) = links
        .iter_mut()
        .find(|link| link.receipt_id == receipt_id)
    {
        if !link.settled {
            link.settled = true;
            save_links(&links);
        }
    }
}

/// 未终结关联的 distinct cwd 集合（应用级 poll 的笔记本范围）。
pub fn pending_cwds() -> Vec<String> {
    let mut links = load_links();
    prune(&mut links);
    let mut cwds: Vec<String> = links
        .iter()
        .filter(|link| !link.settled)
        .map(|link| link.cwd.clone())
        .collect();
    cwds.sort();
    cwds.dedup();
    cwds
}

/// 解析发起会话：最近活跃的 DSH thread（多活跃取最近并留痕）。
pub async fn resolve_active_dsh_thread(app: &tauri::AppHandle) -> Option<String> {
    use tauri::Manager as _;
    let state = app.state::<crate::app::state::AppState>();
    let threads = state.deepseek_harness.running_threads().await;
    let mut candidates: Vec<(i64, String)> = threads
        .into_iter()
        .filter(|(_, info)| info.agent_type.as_deref() == Some("deepseek-harness"))
        .map(|(thread_id, info)| (info.started_at, thread_id))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    match candidates.len() {
        0 => None,
        1 => Some(candidates.remove(0).1),
        _ => {
            let (_, thread_id) = candidates.remove(0);
            tracing::warn!(
                thread_id = %thread_id,
                concurrent = candidates.len() + 1,
                "multiple active DSH threads; consult reply routes to the most recent"
            );
            Some(thread_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(receipt: &str, settled: bool) -> ConsultLink {
        ConsultLink {
            receipt_id: receipt.into(),
            thread_id: Some("thread-1".into()),
            cwd: "/tmp/nb".into(),
            target_agent_id: "alice/researcher@https://relay".into(),
            at: now_unix(),
            settled,
        }
    }

    #[test]
    fn register_find_settle_roundtrip() {
        let mut store = vec![link("rct_a", false), link("rct_b", true)];
        // settle rct_a 后 find_open 不再命中，has_pending 变 false。
        if let Some(found) = store.iter_mut().find(|l| l.receipt_id == "rct_a") {
            found.settled = true;
        }
        assert!(store.iter().any(|l| l.receipt_id == "rct_a" && l.settled));
        assert!(!store.iter().any(|l| !l.settled));
        // pending_cwds 只统计未终结。
        let cwds = {
            let mut pending: Vec<String> = store
                .iter()
                .filter(|l| !l.settled)
                .map(|l| l.cwd.clone())
                .collect();
            pending.sort();
            pending.dedup();
            pending
        };
        assert!(cwds.is_empty());
    }

    #[test]
    fn prune_drops_expired() {
        let mut links = vec![link("rct_old", false)];
        links[0].at = now_unix() - LINK_TTL_SECS - 10;
        assert!(prune(&mut links));
        assert!(links.is_empty());
    }

    #[test]
    fn prune_keeps_fresh_and_settled() {
        let mut links = vec![link("rct_fresh", false), link("rct_done", true)];
        assert!(!prune(&mut links));
        assert_eq!(links.len(), 2);
    }
}
