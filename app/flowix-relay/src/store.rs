//! 中继存储：SQLite WAL 持久化（FD-11 M2）。
//! v1 语义（匿名设备注册 / 收件队列 / TTL / ack）不变；新增账号、
//! 命名空间归属、登录挑战与用量计量。

use std::path::Path;
use std::sync::Mutex;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::envelope::WireEnvelope;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedEnvelope {
    pub queue_id: String,
    pub accepted_at: i64,
    pub envelope: WireEnvelope,
}

/// `alice/researcher` → ("alice", "researcher")；裸 id（`agt_…`）→ None。
/// 命名空间契约见 F5-CROSS-USER-NETWORK §3。
pub fn split_namespaced(agent_id: &str) -> Option<(&str, &str)> {
    let (user, slug) = agent_id.split_once('/')?;
    (!user.is_empty() && !slug.is_empty()).then_some((user, slug))
}

/// 登录挑战有效期。
const CHALLENGE_TTL_SECS: i64 = 300;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// 内存态（测试用）。
    pub fn in_memory() -> Self {
        Self::open(":memory:").expect("sqlite in-memory init failed")
    }

    /// 落盘态：存在则直接恢复（SQLite 即持久层，无导出/导入步骤）。
    pub fn persisted(path: impl AsRef<Path>) -> Self {
        Self::open(path).expect("sqlite open failed")
    }

    fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS accounts(
                user_id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                public_key BLOB NOT NULL,
                created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS devices(
                token TEXT PRIMARY KEY,
                user_id TEXT,
                created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS agent_owner(
                agent_id TEXT PRIMARY KEY,
                token TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS queue(
                queue_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                accepted_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                envelope_json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS idx_queue_agent ON queue(agent_id);
            CREATE TABLE IF NOT EXISTS usage_events(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subject TEXT NOT NULL,
                action TEXT NOT NULL,
                at INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS idx_usage_subject
                ON usage_events(subject, action, at);
            CREATE TABLE IF NOT EXISTS challenges(
                challenge TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                expires_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS directory(
                user_id TEXT NOT NULL,
                username TEXT NOT NULL,
                agent_slug TEXT NOT NULL,
                display_name TEXT NOT NULL,
                visibility TEXT NOT NULL,
                card_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(user_id, agent_slug));
            CREATE INDEX IF NOT EXISTS idx_directory_search
                ON directory(visibility, updated_at);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ------------------------------------------------------------------
    // 账号（FD-11 §3/§5：密钥即身份，无密码）
    // ------------------------------------------------------------------

    /// 注册账号：username 唯一，公钥为 Ed25519 VerifyKey 字节。
    /// 返回 (userId, deviceToken)。username 已存在时返回 None（调用方走挑战登录）。
    pub fn create_account(&self, username: &str, public_key: &[u8], now: i64) -> Option<(String, String)> {
        let user_id = format!("usr_{}", uuid::Uuid::new_v4().simple());
        let mut conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO accounts(user_id, username, public_key, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![user_id, username, public_key, now],
        )
        .ok()?;
        let token = Self::insert_device_locked(&mut conn, Some(&user_id), now);
        Some((user_id, token))
    }

    pub fn account_by_username(&self, username: &str) -> Option<(String, Vec<u8>)> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT user_id, public_key FROM accounts WHERE username = ?1",
                [username],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap_or_none_from_err()
    }

    /// 签发一次性登录挑战（旧挑战作废）。
    pub fn issue_challenge(&self, user_id: &str, now: i64) -> String {
        let challenge = format!("chl_{}", uuid::Uuid::new_v4().simple());
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM challenges WHERE user_id = ?1", [user_id]).ok();
        conn.execute(
            "INSERT INTO challenges(challenge, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![challenge, user_id, now + CHALLENGE_TTL_SECS],
        )
        .ok();
        challenge
    }

    /// 挑战登录：验证 Ed25519 签名后换发新 deviceToken。
    /// 返回 Err 原因串供路由映射 401 细分码。
    pub fn redeem_challenge(&self, user_id: &str, challenge: &str, signature: &[u8], now: i64) -> Result<String, &'static str> {
        let mut conn = self.conn.lock().unwrap();
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT user_id, expires_at FROM challenges WHERE challenge = ?1",
                [challenge],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| "internal")?;
        let Some((owner, expires_at)) = row else { return Err("CHALLENGE_INVALID") };
        if owner != user_id || expires_at <= now {
            return Err("CHALLENGE_INVALID");
        }
        let public_key: Vec<u8> = conn
            .query_row("SELECT public_key FROM accounts WHERE user_id = ?1", [user_id], |row| row.get(0))
            .optional()
            .map_err(|_| "internal")?
            .ok_or("CHALLENGE_INVALID")?;
        let public_key_bytes: &[u8; 32] =
            public_key.as_slice().try_into().map_err(|_| "SIGNATURE_INVALID")?;
        let verifying_key =
            VerifyingKey::from_bytes(public_key_bytes).map_err(|_| "SIGNATURE_INVALID")?;
        let signature =
            Signature::from_slice(signature).map_err(|_| "SIGNATURE_INVALID")?;
        verifying_key
            .verify(challenge.as_bytes(), &signature)
            .map_err(|_| "SIGNATURE_INVALID")?;
        conn.execute("DELETE FROM challenges WHERE challenge = ?1", [challenge]).ok();
        Ok(Self::insert_device_locked(&mut conn, Some(user_id), now))
    }

    /// token → 归属账号 (userId, username)（命名空间前缀来源）。
    pub fn account_of_token(&self, token: &str) -> Option<(String, String)> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT a.user_id, a.username FROM devices d JOIN accounts a ON a.user_id = d.user_id
                 WHERE d.token = ?1",
                [token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap_or(None)
    }

    fn insert_device_locked(conn: &mut Connection, user_id: Option<&str>, now: i64) -> String {
        let token = format!("dev_{}", uuid::Uuid::new_v4().simple());
        conn.execute(
            "INSERT INTO devices(token, user_id, created_at) VALUES (?1, ?2, ?3)",
            params![token, user_id, now],
        )
        .ok();
        token
    }

    // ------------------------------------------------------------------
    // 设备与 agent 归属（v1 语义 + v2 账号归属）
    // ------------------------------------------------------------------

    /// 注册收件设备；返回 deviceToken。后注册的设备夺走 agent 的收件权（单设备语义）。
    /// `user_id`：Some = v2 账号设备（agentId 由路由层做命名空间归一）；None = v1 匿名。
    pub fn register_device(&self, agent_ids: Vec<String>, user_id: Option<&str>, now: i64) -> String {
        let mut conn = self.conn.lock().unwrap();
        let token = Self::insert_device_locked(&mut conn, user_id, now);
        for agent_id in agent_ids {
            conn.execute("DELETE FROM agent_owner WHERE agent_id = ?1", [&agent_id]).ok();
            conn.execute(
                "INSERT INTO agent_owner(agent_id, token) VALUES (?1, ?2)",
                params![agent_id, token],
            )
            .ok();
        }
        token
    }

    pub fn token_owns_agent(&self, token: &str, agent_id: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT token FROM agent_owner WHERE agent_id = ?1",
                [agent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap_or(None)
            .is_some_and(|owner| owner == token)
    }

    pub fn agent_registered(&self, agent_id: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT 1 FROM agent_owner WHERE agent_id = ?1", [agent_id], |_| Ok(()))
            .optional()
            .unwrap_or(Some(()))
            .is_some()
    }

    // ------------------------------------------------------------------
    // 收件队列（v1 语义不变：at-least-once + ack 出队 + TTL 清理）
    // ------------------------------------------------------------------

    /// 入队（调用前已过结构校验）；返回 queueId。
    pub fn enqueue(&self, agent_id: &str, envelope: WireEnvelope, now_unix: i64) -> String {
        let queue_id = format!("q_{}", uuid::Uuid::new_v4().simple());
        let expires_at = envelope.disclosure.expires_at;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO queue(queue_id, agent_id, accepted_at, expires_at, envelope_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![queue_id, agent_id, now_unix, expires_at, serde_json::to_string(&envelope).expect("envelope serializable")],
            )
            .ok();
        queue_id
    }

    /// 待投递列表（不移除；ack 才出队），同时清掉已过期信封。
    pub fn inbox(&self, agent_id: &str, now_unix: i64) -> Vec<QueuedEnvelope> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM queue WHERE agent_id = ?1 AND expires_at <= ?2", params![agent_id, now_unix]).ok();
        let mut stmt = conn
            .prepare("SELECT queue_id, accepted_at, envelope_json FROM queue WHERE agent_id = ?1 ORDER BY accepted_at, queue_id")
            .expect("inbox select prepare");
        stmt.query_map([agent_id], |row| {
            let queue_id: String = row.get(0)?;
            let accepted_at: i64 = row.get(1)?;
            let envelope_json: String = row.get(2)?;
            Ok(serde_json::from_str::<WireEnvelope>(&envelope_json).map(|envelope| QueuedEnvelope {
                queue_id,
                accepted_at,
                envelope,
            }))
        })
        .expect("inbox query")
        .filter_map(Result::ok)
        .filter_map(Result::ok)
        .collect()
    }

    /// ack 出队；返回实际移除的数量（重复 ack 幂等返回 0）。
    pub fn ack(&self, agent_id: &str, queue_ids: &[String]) -> usize {
        let conn = self.conn.lock().unwrap();
        queue_ids
            .iter()
            .filter_map(|queue_id| {
                conn.execute(
                    "DELETE FROM queue WHERE queue_id = ?1 AND agent_id = ?2",
                    params![queue_id, agent_id],
                )
                .ok()
            })
            .sum()
    }

    // ------------------------------------------------------------------
    // 目录（FD-11 §4：公开卡与三档可见性）
    // ------------------------------------------------------------------

    /// 发布/更新公开卡（调用前已校验归属与字段合法性）。
    pub fn upsert_directory_card(
        &self,
        user_id: &str,
        username: &str,
        agent_slug: &str,
        display_name: &str,
        visibility: &str,
        card_json: &str,
        now: i64,
    ) {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO directory(user_id, username, agent_slug, display_name, visibility, card_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(user_id, agent_slug) DO UPDATE SET
                    username = excluded.username,
                    display_name = excluded.display_name,
                    visibility = excluded.visibility,
                    card_json = excluded.card_json,
                    updated_at = excluded.updated_at",
                params![user_id, username, agent_slug, display_name, visibility, card_json, now],
            )
            .ok();
    }

    /// 搜索：仅 `public` 卡；q 匹配 username/displayName/卡片内容，tag 匹配卡片标签。
    pub fn search_directory(&self, q: &str, tag: &str, limit: i64, offset: i64) -> Vec<DirectoryRow> {
        let q_trim = q.trim();
        let pattern = format!("%{}%", q_trim.to_lowercase());
        let tag_trim = tag.trim();
        let tag_pattern = format!("%\"{}\"%", tag_trim);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT username, agent_slug, display_name, visibility, card_json, updated_at
                 FROM directory
                 WHERE visibility = 'public'
                   AND (?3 OR lower(username) LIKE ?1
                        OR lower(display_name) LIKE ?1 OR lower(card_json) LIKE ?1)
                   AND (?4 OR card_json LIKE ?2)
                 ORDER BY updated_at DESC, username, agent_slug
                 LIMIT ?5 OFFSET ?6",
            )
            .expect("directory search prepare");
        stmt.query_map(
            params![pattern, tag_pattern, q_trim.is_empty(), tag_trim.is_empty(), limit, offset],
            row_to_directory,
        )
        .expect("directory search query")
        .filter_map(Result::ok)
        .collect()
    }

    /// 单卡详情：任意可见性（private 由路由层拒绝）。
    pub fn directory_card(&self, username: &str, agent_slug: &str) -> Option<DirectoryRow> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT username, agent_slug, display_name, visibility, card_json, updated_at
                 FROM directory WHERE username = ?1 AND agent_slug = ?2",
                params![username, agent_slug],
                row_to_directory,
            )
            .optional()
            .unwrap_or(None)
    }

    /// 投递执法（§4）：private 卡的 agent 拒收跨用户信封。
    pub fn directory_visibility_of_agent(&self, agent_id: &str) -> Option<String> {
        let (username, slug) = split_namespaced(agent_id)?;
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT visibility FROM directory WHERE username = ?1 AND agent_slug = ?2",
                params![username, slug],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None)
    }

    /// 验签信任锚（§5）：from agent 目录卡上的 base64 公钥。
    pub fn directory_public_key_of_agent(&self, agent_id: &str) -> Option<String> {
        let row = self.directory_card_of_agent(agent_id)?;
        let card: serde_json::Value = serde_json::from_str(&row).ok()?;
        card.get("publicKey").and_then(|value| value.as_str()).map(str::to_string)
    }

    fn directory_card_of_agent(&self, agent_id: &str) -> Option<String> {
        let (username, slug) = split_namespaced(agent_id)?;
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT card_json FROM directory WHERE username = ?1 AND agent_slug = ?2",
                params![username, slug],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None)
    }

    // ------------------------------------------------------------------
    // 用量计量（FD-11 §5：配额执法依据）
    // ------------------------------------------------------------------

    pub fn record_usage(&self, subject: &str, action: &str, now: i64) {
        self.conn
            .lock()
            .unwrap()
            .execute("INSERT INTO usage_events(subject, action, at) VALUES (?1, ?2, ?3)", params![subject, action, now])
            .ok();
    }

    pub fn count_usage(&self, subject: &str, action: &str, since: i64) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE subject = ?1 AND action = ?2 AND at >= ?3",
                params![subject, action, since],
                |row| row.get(0),
            )
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRow {
    pub username: String,
    pub agent_slug: String,
    pub display_name: String,
    pub visibility: String,
    pub card_json: String,
    pub updated_at: i64,
}

fn row_to_directory(row: &rusqlite::Row<'_>) -> rusqlite::Result<DirectoryRow> {
    Ok(DirectoryRow {
        username: row.get(0)?,
        agent_slug: row.get(1)?,
        display_name: row.get(2)?,
        visibility: row.get(3)?,
        card_json: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// rusqlite optional 辅助：Err 降级 None（读路径容错，与 v1 JSON 恢复语义一致）。
trait OptionalExt<T> {
    fn unwrap_or_none_from_err(self) -> Option<T>;
}
impl<T> OptionalExt<T> for Result<Option<T>, rusqlite::Error> {
    fn unwrap_or_none_from_err(self) -> Option<T> {
        self.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::tests::sample;
    use ed25519_dalek::Signer;

    #[test]
    fn register_and_route() {
        let store = Store::in_memory();
        let token = store.register_device(vec!["agt_b".into()], None, 1);
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
        let first = store.register_device(vec!["agt_b".into()], None, 1);
        let second = store.register_device(vec!["agt_b".into()], None, 1);
        assert!(!store.token_owns_agent(&first, "agt_b"));
        assert!(store.token_owns_agent(&second, "agt_b"));
    }

    #[test]
    fn expired_envelopes_are_dropped_from_inbox() {
        let store = Store::in_memory();
        store.register_device(vec!["agt_b".into()], None, 1);
        store.enqueue("agt_b", sample(), 1);
        // sample 的 expires_at = 4_102_444_800。
        assert!(store.inbox("agt_b", 4_102_444_900).is_empty());
    }

    #[test]
    fn persisted_state_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay-state.db");
        {
            let store = Store::persisted(&path);
            store.register_device(vec!["agt_b".into()], None, 1);
            store.enqueue("agt_b", sample(), 1);
        }
        let restored = Store::persisted(&path);
        assert!(restored.agent_registered("agt_b"));
        assert_eq!(restored.inbox("agt_b", 1).len(), 1);
    }

    #[test]
    fn account_challenge_and_namespaced_registration() {
        let store = Store::in_memory();
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let (user_id, _token) =
            store.create_account("alice", key.verifying_key().as_bytes(), 1).unwrap();
        let challenge = store.issue_challenge(&user_id, 1);
        let signature = key.sign(challenge.as_bytes());
        let login_token =
            store.redeem_challenge(&user_id, &challenge, &signature.to_bytes(), 1).unwrap();
        assert_eq!(
            store.account_of_token(&login_token).map(|(_, username)| username),
            Some("alice".to_string())
        );

        // 命名空间 agent 注册（归属 alice）。
        let agent_token = store.register_device(vec!["alice/researcher".into()], Some(&user_id), 1);
        assert!(store.agent_registered("alice/researcher"));
        assert!(store.token_owns_agent(&agent_token, "alice/researcher"));

        // 挑战一次性：重放被拒。
        let replay = store.redeem_challenge(&user_id, &challenge, &signature.to_bytes(), 1);
        assert_eq!(replay.unwrap_err(), "CHALLENGE_INVALID");
    }

    #[test]
    fn wrong_key_cannot_redeem_challenge() {
        let store = Store::in_memory();
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let (user_id, _) = store.create_account("alice", key.verifying_key().as_bytes(), 1).unwrap();
        let challenge = store.issue_challenge(&user_id, 1);
        let attacker = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        let signature = attacker.sign(challenge.as_bytes());
        assert_eq!(
            store.redeem_challenge(&user_id, &challenge, &signature.to_bytes(), 1).unwrap_err(),
            "SIGNATURE_INVALID"
        );
    }

    #[test]
    fn usage_counter_window() {
        let store = Store::in_memory();
        store.record_usage("dev_x", "deliver", 100);
        store.record_usage("dev_x", "deliver", 200);
        assert_eq!(store.count_usage("dev_x", "deliver", 150), 1);
        assert_eq!(store.count_usage("dev_x", "deliver", 0), 2);
    }

    #[test]
    fn namespace_split() {
        assert_eq!(split_namespaced("alice/researcher"), Some(("alice", "researcher")));
        assert_eq!(split_namespaced("agt_b"), None);
        assert_eq!(split_namespaced("alice/"), None);
        assert_eq!(split_namespaced("/researcher"), None);
    }
}
