//! 中继暂存：设备注册、按 agent 收件队列、TTL 清理与落盘恢复。
//! v1 单进程；内存态 + 每次变更后整体落盘（量级为个位数信封）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::envelope::WireEnvelope;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedEnvelope {
    pub queue_id: String,
    pub accepted_at: i64,
    pub envelope: WireEnvelope,
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedState {
    /// deviceToken -> 注册的 agent 集合。
    devices: HashMap<String, Vec<String>>,
    /// agentId -> deviceToken（一个 agent 绑定一台设备，后注册覆盖）。
    agent_owner: HashMap<String, String>,
    /// agentId -> 待投递队列。
    queues: HashMap<String, Vec<QueuedEnvelope>>,
}

pub struct Store {
    path: Option<PathBuf>,
    inner: Mutex<PersistedState>,
}

impl Store {
    /// 内存态（测试用）。
    pub fn in_memory() -> Self {
        Self { path: None, inner: Mutex::new(PersistedState::default()) }
    }

    /// 落盘态：存在则先恢复。
    pub fn persisted(path: PathBuf) -> Self {
        let inner = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PersistedState>(&bytes).ok())
            .unwrap_or_default();
        Self { path: Some(path), inner: Mutex::new(inner) }
    }

    fn flush_locked(state: &PersistedState, path: &Option<PathBuf>) {
        if let Some(path) = path {
            if let Ok(bytes) = serde_json::to_vec_pretty(state) {
                // 先写临时再原子替换，避免断电留下半截状态。
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, &bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, path);
                }
            }
        }
    }

    /// 注册设备；返回 deviceToken。后注册的设备夺走 agent 的收件权（单设备语义）。
    pub fn register_device(&self, agent_ids: Vec<String>) -> String {
        let token = format!("dev_{}", uuid::Uuid::new_v4().simple());
        let mut state = self.inner.lock().unwrap();
        for agent_id in agent_ids {
            if let Some(previous) = state.agent_owner.insert(agent_id.clone(), token.clone()) {
                state.devices.entry(previous).and_modify(|list| list.retain(|id| id != &agent_id));
            }
            state.devices.entry(token.clone()).or_default().push(agent_id);
        }
        Self::flush_locked(&state, &self.path);
        token
    }

    pub fn token_owns_agent(&self, token: &str, agent_id: &str) -> bool {
        let state = self.inner.lock().unwrap();
        state.agent_owner.get(agent_id).map(String::as_str) == Some(token)
    }

    pub fn agent_registered(&self, agent_id: &str) -> bool {
        let state = self.inner.lock().unwrap();
        state.agent_owner.contains_key(agent_id)
    }

    /// 入队（调用前已过结构校验）；返回 queueId。
    pub fn enqueue(&self, agent_id: &str, envelope: WireEnvelope, now_unix: i64) -> String {
        let queue_id = format!("q_{}", uuid::Uuid::new_v4().simple());
        let mut state = self.inner.lock().unwrap();
        state.queues.entry(agent_id.to_string()).or_default().push(QueuedEnvelope {
            queue_id: queue_id.clone(),
            accepted_at: now_unix,
            envelope,
        });
        Self::flush_locked(&state, &self.path);
        queue_id
    }

    /// 待投递列表（不移除；ack 才出队），同时清掉已过期信封。
    pub fn inbox(&self, agent_id: &str, now_unix: i64) -> Vec<QueuedEnvelope> {
        let mut state = self.inner.lock().unwrap();
        if let Some(queue) = state.queues.get_mut(agent_id) {
            queue.retain(|item| item.envelope.disclosure.expires_at > now_unix);
            let snapshot = queue.clone();
            Self::flush_locked(&state, &self.path);
            snapshot
        } else {
            Vec::new()
        }
    }

    /// ack 出队；返回实际移除的数量（重复 ack 幂等返回 0）。
    pub fn ack(&self, agent_id: &str, queue_ids: &[String]) -> usize {
        let mut state = self.inner.lock().unwrap();
        let removed = state
            .queues
            .get_mut(agent_id)
            .map(|queue| {
                let before = queue.len();
                queue.retain(|item| !queue_ids.contains(&item.queue_id));
                before - queue.len()
            })
            .unwrap_or(0);
        if removed > 0 {
            Self::flush_locked(&state, &self.path);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::tests::sample;

    #[test]
    fn register_and_route() {
        let store = Store::in_memory();
        let token = store.register_device(vec!["agt_b".into()]);
        assert!(store.agent_registered("agt_b"));
        assert!(store.token_owns_agent(&token, "agt_b"));

        store.enqueue("agt_b", sample(), 1);
        let inbox = store.inbox("agt_b", 1);
        assert_eq!(inbox.len(), 1);
        store.ack("agt_b", &[inbox[0].queue_id.clone()]);
        assert!(store.inbox("agt_b", 1).is_empty());
        // 幂等 ack。
        assert_eq!(store.ack("agt_b", &[inbox[0].queue_id.clone()]), 0);
    }

    #[test]
    fn reregister_steals_agent() {
        let store = Store::in_memory();
        let first = store.register_device(vec!["agt_b".into()]);
        let second = store.register_device(vec!["agt_b".into()]);
        assert!(!store.token_owns_agent(&first, "agt_b"));
        assert!(store.token_owns_agent(&second, "agt_b"));
    }

    #[test]
    fn expired_envelopes_are_dropped_from_inbox() {
        let store = Store::in_memory();
        store.register_device(vec!["agt_b".into()]);
        store.enqueue("agt_b", sample(), 1);
        // sample 的 expires_at = 4_102_444_800。
        assert!(store.inbox("agt_b", 4_102_444_900).is_empty());
    }

    #[test]
    fn persisted_state_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay-state.json");
        {
            let store = Store::persisted(path.clone());
            store.register_device(vec!["agt_b".into()]);
            store.enqueue("agt_b", sample(), 1);
        }
        let restored = Store::persisted(path);
        assert!(restored.agent_registered("agt_b"));
        assert_eq!(restored.inbox("agt_b", 1).len(), 1);
    }
}
