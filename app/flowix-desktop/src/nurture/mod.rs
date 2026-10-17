//! F2 喂养管道 N1 骨架：触发器 + 合并调度器 + 留痕。
//!
//! 只覆盖 F2 详细设计 v2.0 §3.1/§3.2 的 N1 范围：事件分发、路径与 scope
//! 双过滤、阈值计数、coalescing 调度与 skip/failed 留痕。取材、蒸馏与写回
//! 属 N2/N3，不在本模块。
//!
//! 骨架阶段：`start_trigger`/`start_scheduler` 待 AppState 接线，`source`/
//! `updated_at`/`NurtureState` 等字段为 N2/N3 预留，故整体允许 dead_code。

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::config::access::{AgentAccessEntry, AgentAccessKind};
use crate::memo_events::MemoChangeSource;

/// 触发器收到的事件，由 `memo_events::emit` 主路径派生并分发。
#[derive(Debug, Clone)]
pub struct NurtureTriggerEvent {
    pub source: MemoChangeSource,
    pub notebook_id: String,
    pub relative_path: String,
    pub tags: Vec<String>,
    pub updated_at: i64,
}

/// 进程内事件通道。前端 Tauri 事件不变，这里只多一路内部分发；无订阅者
/// 时发送即丢弃。
fn trigger_channel() -> &'static tokio::sync::broadcast::Sender<NurtureTriggerEvent> {
    static CHANNEL: OnceLock<tokio::sync::broadcast::Sender<NurtureTriggerEvent>> =
        OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (tx, _rx) = tokio::sync::broadcast::channel(256);
        tx
    })
}

/// 由 `memo_events` 调用：把单条 memo 事件派发给触发器订阅者。
pub fn dispatch_trigger_event(event: NurtureTriggerEvent) {
    let _ = trigger_channel().send(event);
}

/// 启动触发器订阅 task。`scopes_provider` 每次事件时被调用，返回该
/// notebook 的授权条目（Notebook/Folder/Tag）。
///
/// 待 N1 集成接线：需从 AppState 注入 AgentAccessStore 闭包后在 bootstrap
/// 中 spawn，当前 N1 骨架阶段未接线，故标记允许 dead_code。
#[allow(dead_code)]
pub fn start_trigger<F>(scopes_provider: F) -> tokio::task::JoinHandle<()>
where
    F: Fn(&str) -> Vec<AgentAccessEntry> + Send + Sync + 'static,
{
    let mut rx = trigger_channel().subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let scopes = scopes_provider(&event.notebook_id);
            if !should_accept_event(&event.relative_path, &event.tags, &scopes) {
                continue;
            }
            let mut sched = scheduler().lock().unwrap_or_else(|e| e.into_inner());
            let _ = sched.count_and_enqueue(event, /*manual=*/ false);
        }
    })
}

/// 纯函数：路径 + scope 双过滤。
///
/// 返回 false 表示该事件不应进入喂养计数。第一条闸：`.agents/` 与
/// `.flowix/` 路径直接丢弃（防写回自身产物触发循环）。第二条闸：事件路径
/// 必须命中至少一个 enabled 授权条目。
pub fn should_accept_event(
    relative_path: &str,
    tags: &[String],
    scopes: &[AgentAccessEntry],
) -> bool {
    if relative_path.starts_with(".agents/") || relative_path.starts_with(".flowix/") {
        return false;
    }
    scopes
        .iter()
        .filter(|entry| entry.enabled)
        .any(|entry| entry_matches(entry, relative_path, tags))
}

fn entry_matches(entry: &AgentAccessEntry, relative_path: &str, tags: &[String]) -> bool {
    match entry.kind {
        AgentAccessKind::Notebook => true,
        AgentAccessKind::Folder => {
            let prefix = entry.path.trim_end_matches(['/', '\\']);
            relative_path.starts_with(prefix)
                && relative_path
                    .get(prefix.len()..)
                    .map(|rest| rest.starts_with('/') || rest.is_empty())
                    .unwrap_or(true)
        }
        AgentAccessKind::Tag => {
            let Some(tag_path) = entry.tag_path.as_deref() else {
                return false;
            };
            tags.iter().any(|tag| {
                tag == tag_path
                    || tag
                        .strip_prefix(tag_path)
                        .map(|rest| rest.starts_with('/'))
                        .unwrap_or(false)
            })
        }
    }
}

// ---------------------------------------------------------------------------
// 调度器（coalescing worker，对照 document_derived.rs 模式）
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Scheduler {
    running: bool,
    counters: BTreeMap<String, usize>, // agent_id -> 素材计数
    jobs: BTreeMap<String, Job>,       // agent_id -> Job
}

struct Job {
    notebook_id: String,
    agent_id: String,
    manual: bool,
}

fn scheduler() -> &'static Mutex<Scheduler> {
    static SCHEDULER: OnceLock<Mutex<Scheduler>> = OnceLock::new();
    SCHEDULER.get_or_init(|| Mutex::new(Scheduler::default()))
}

impl Scheduler {
    /// 计数并判断是否达阈值。达阈值清零并入队，返回 true 表示已入队。
    fn count_and_enqueue(&mut self, event: NurtureTriggerEvent, manual: bool) -> bool {
        // N1 骨架：agent_id 暂用 notebook_id 派生占位，真实映射在 F1/F2
        // 全量接线时替换为「scope 覆盖该笔记的所有 agent」。
        let agent_id = format!("nurture:{}", event.notebook_id);
        let counter = self.counters.entry(agent_id.clone()).or_insert(0);
        *counter += 1;
        if *counter >= DEFAULT_THRESHOLD {
            *counter = 0;
            self.jobs.entry(agent_id.clone()).or_insert(Job {
                notebook_id: event.notebook_id,
                agent_id,
                manual,
            });
            return true;
        }
        false
    }

    fn pop_next(&mut self) -> Option<Job> {
        self.jobs.pop_first().map(|(_key, job)| job)
    }
}

const DEFAULT_THRESHOLD: usize = 10;

/// 调度器工作循环入口。由 bootstrap 调用，spawn 单循环消费 jobs。
///
/// 待 N1 集成接线：需从 AppState 派生 nurture-log 目录后 spawn，当前 N1
/// 骨架阶段未接线，故标记允许 dead_code。
#[allow(dead_code)]
pub fn start_scheduler<F>(log_dir: F) -> tokio::task::JoinHandle<()>
where
    F: Fn(&str) -> PathBuf + Send + Sync + 'static,
{
    tokio::spawn(async move {
        loop {
            let job = {
                let mut sched = scheduler().lock().unwrap_or_else(|e| e.into_inner());
                sched.pop_next()
            };
            let Some(job) = job else {
                tokio::task::yield_now().await;
                continue;
            };
            // N1 骨架：跳过判断与留痕。无实际取材/蒸馏。
            append_log(&log_dir(&job.agent_id), &NurtureLogLine::skipped("no_material"));
        }
    })
}

// ---------------------------------------------------------------------------
// 留痕（nurture.json + nurture-log.jsonl）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NurtureState {
    pub version: u32,
    pub agent_id: String,
    pub watermark: i64,
    pub last_run_at: Option<i64>,
    pub consecutive_failures: u32,
    pub paused: bool,
    pub tokens_total: u64,
}

impl Default for NurtureState {
    fn default() -> Self {
        Self {
            version: 1,
            agent_id: String::new(),
            watermark: 0,
            last_run_at: None,
            consecutive_failures: 0,
            paused: false,
            tokens_total: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NurtureLogLine {
    pub ts: i64,
    pub kind: String, // started | succeeded | failed | skipped
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub material_count: usize,
    #[serde(default)]
    pub truncated: usize,
}

impl NurtureLogLine {
    pub fn skipped(reason: &str) -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            kind: "skipped".into(),
            reason: Some(reason.into()),
            material_count: 0,
            truncated: 0,
        }
    }
}

fn append_log(dir: &PathBuf, line: &NurtureLogLine) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("nurture-log.jsonl");
    let encoded = serde_json::to_string(line).unwrap_or_default();
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{encoded}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str) -> AgentAccessEntry {
        AgentAccessEntry {
            id: "fld".into(),
            kind: AgentAccessKind::Folder,
            path: path.into(),
            name: "Folder".into(),
            enabled: true,
            tag_path: None,
            workspace: false,
            added_at: 1,
            updated_at: 1,
            missing: false,
        }
    }

    fn tag(path: &str) -> AgentAccessEntry {
        AgentAccessEntry {
            id: "tag".into(),
            kind: AgentAccessKind::Tag,
            path: String::new(),
            name: "Tag".into(),
            enabled: true,
            tag_path: Some(path.into()),
            workspace: false,
            added_at: 1,
            updated_at: 1,
            missing: false,
        }
    }

    #[test]
    fn rejects_dot_agents_and_dot_flowix_paths() {
        assert!(!should_accept_event(".agents/x/profile.md", &[], &[folder("/tmp")]));
        assert!(!should_accept_event(".flowix/state.json", &[], &[folder("/tmp")]));
    }

    #[test]
    fn rejects_path_outside_all_scopes() {
        let scopes = vec![folder("/tmp/notebook")];
        assert!(!should_accept_event("other/file.md", &[], &scopes));
    }

    #[test]
    fn folder_scope_accepts_prefix() {
        let scopes = vec![folder("/tmp/notebook")];
        assert!(should_accept_event("/tmp/notebook/a.md", &[], &scopes));
        assert!(should_accept_event("/tmp/notebook/sub/b.md", &[], &scopes));
    }

    #[test]
    fn tag_scope_accepts_exact_and_subtree() {
        let scopes = vec![tag("finance/财报")];
        assert!(should_accept_event("a.md", &["finance/财报".into()], &scopes));
        assert!(should_accept_event(
            "b.md",
            &["finance/财报/q3".into()],
            &scopes
        ));
        assert!(!should_accept_event("c.md", &["industry/电商".into()], &scopes));
    }

    #[test]
    fn disabled_scope_is_ignored() {
        let mut entry = tag("finance/财报");
        entry.enabled = false;
        assert!(!should_accept_event(
            "a.md",
            &["finance/财报".into()],
            &[entry]
        ));
    }

    #[test]
    fn counter_reaches_threshold_then_resets() {
        let mut sched = Scheduler::default();
        for _ in 0..DEFAULT_THRESHOLD - 1 {
            assert!(!sched.count_and_enqueue(ev("a.md"), false));
        }
        assert!(sched.count_and_enqueue(ev("a.md"), false));
        // 清零后重新计数。
        assert!(!sched.count_and_enqueue(ev("a.md"), false));
    }

    #[test]
    fn scheduler_dedupes_jobs_by_agent() {
        let mut sched = Scheduler::default();
        for _ in 0..DEFAULT_THRESHOLD * 2 {
            sched.count_and_enqueue(ev("a.md"), false);
        }
        // 两次达阈值，但同一 agent 只保留一个 job。
        assert_eq!(sched.jobs.len(), 1);
    }

    fn ev(relative_path: &str) -> NurtureTriggerEvent {
        NurtureTriggerEvent {
            source: MemoChangeSource::UserEdit,
            notebook_id: "nb".into(),
            relative_path: relative_path.into(),
            tags: vec!["finance/财报".into()],
            updated_at: 1,
        }
    }
}
