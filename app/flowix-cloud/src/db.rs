//! SQLite 存储层（rusqlite bundled；单文件库）。

use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct CloudDb {
    connection: Mutex<Connection>,
}

/// 短临界区、锁内无 await：std::sync::Mutex 足够。
pub struct DbGuard<'a>(pub &'a CloudDb);

impl CloudDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path)
            .map_err(|error| format!("open sqlite {path:?}: {error}", path = path.display()))?;
        Self::init(connection)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn init(connection: Connection) -> Result<Self, String> {
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("init schema: {e}"))?;
        // 幂等列迁移：旧库的 changes 表没有 thread 的 agent_id 列。
        // "duplicate column name" 视为已迁移。
        if let Err(error) = connection.execute_batch("ALTER TABLE changes ADD COLUMN agent_id TEXT") {
            let message = error.to_string();
            if !message.contains("duplicate column") {
                return Err(format!("migrate changes.agent_id: {message}"));
            }
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn with<T>(&self, run: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let guard = self.connection.lock().map_err(|_| "db mutex poisoned")?;
        run(&guard)
    }

    /// 事务包裹（node 版语义：push 逐操作，任何 panic 性错误整体回滚）。
    pub fn with_transaction<T>(
        &self,
        run: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let guard = self.connection.lock().map_err(|_| "db mutex poisoned")?;
        guard
            .execute_batch("BEGIN")
            .map_err(|e| format!("begin: {e}"))?;
        match run(&guard) {
            Ok(value) => {
                guard.execute_batch("COMMIT").map_err(|e| format!("commit: {e}"))?;
                Ok(value)
            }
            Err(error) => {
                let _ = guard.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}

pub fn new_revision() -> String {
    format!("rev_{}", uuid::Uuid::now_v7().simple())
}

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  email TEXT UNIQUE NOT NULL,
  password_hash TEXT NOT NULL,
  display_name TEXT NOT NULL,
  system_role TEXT NOT NULL DEFAULT 'user',
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS refresh_tokens (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS access_tokens (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS notebooks (
  user_id TEXT NOT NULL,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  icon TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  revision TEXT NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_seq INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, id)
);
CREATE TABLE IF NOT EXISTS notes (
  user_id TEXT NOT NULL,
  id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  filename TEXT NOT NULL,
  moved_from_note_id TEXT,
  revision TEXT NOT NULL,
  content_hash TEXT,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0,
  attachments TEXT NOT NULL DEFAULT '[]',
  sync_seq INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, id)
);
CREATE TABLE IF NOT EXISTS note_revisions (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  note_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  revision TEXT NOT NULL,
  deleted INTEGER NOT NULL,
  relative_path TEXT,
  content_hash TEXT,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_revisions_note ON note_revisions(user_id, note_id, seq DESC);
CREATE TABLE IF NOT EXISTS changes (
  sync_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  notebook_id TEXT,
  revision TEXT NOT NULL,
  kind TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  name TEXT, icon TEXT, sort_order INTEGER,
  filename TEXT, content_hash TEXT, size_bytes INTEGER,
  deleted INTEGER NOT NULL DEFAULT 0,
  attachments TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_changes_user_seq ON changes(user_id, sync_seq);
CREATE TABLE IF NOT EXISTS blobs (
  content_hash TEXT PRIMARY KEY,
  size_bytes INTEGER NOT NULL,
  content_type TEXT NOT NULL,
  blob_kind TEXT NOT NULL,
  data BLOB NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blob_reservations (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  blob_kind TEXT NOT NULL,
  content_type TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS threads (
  user_id TEXT NOT NULL,
  id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  title TEXT NOT NULL,
  revision TEXT NOT NULL,
  content_hash TEXT,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_seq INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, id)
);
CREATE TABLE IF NOT EXISTS thread_revisions (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  revision TEXT NOT NULL,
  deleted INTEGER NOT NULL,
  title TEXT,
  content_hash TEXT,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_thread_revisions ON thread_revisions(user_id, thread_id, seq DESC);
"#;

/// 变更行 → 客户端 `V2Change` 载荷列。
pub struct ChangeRow {
    pub entity_type: &'static str,
    pub entity_id: String,
    pub notebook_id: Option<String>,
    pub revision: String,
    pub kind: &'static str,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub sort_order: Option<i64>,
    pub filename: Option<String>,
    /// thread 实体的 agent 标识（notebook/note 恒为 None）。
    pub agent_id: Option<String>,
    pub content_hash: Option<String>,
    pub size_bytes: Option<i64>,
    pub deleted: bool,
    pub attachments: String,
}

/// 写入变更流并返回新 sync_seq（即 AUTOINCREMENT rowid）。
pub fn record_change(connection: &Connection, user_id: &str, row: ChangeRow) -> Result<i64, String> {
    connection
        .execute(
            "INSERT INTO changes
              (user_id, entity_type, entity_id, notebook_id, revision, kind, created_at,
               name, icon, sort_order, filename, agent_id, content_hash, size_bytes, deleted, attachments)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            rusqlite::params![
                user_id,
                row.entity_type,
                row.entity_id,
                row.notebook_id,
                row.revision,
                row.kind,
                crate::now_ms(),
                row.name,
                row.icon,
                row.sort_order,
                row.filename,
                row.agent_id,
                row.content_hash,
                row.size_bytes,
                row.deleted as i64,
                row.attachments,
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(connection.last_insert_rowid())
}

pub fn user_head(connection: &Connection, user_id: &str) -> i64 {
    connection
        .query_row(
            "SELECT COALESCE(MAX(sync_seq), 0) FROM changes WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

pub fn user_usage(connection: &Connection, user_id: &str) -> (i64, i64) {
    connection
        .query_row(
            "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*)
               FROM notes WHERE user_id = ?1 AND deleted = 0",
            [user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0, 0))
}

/// 自建版配额：100GB（权益端点用）。
pub const QUOTA_BYTES: i64 = 100 * 1024 * 1024 * 1024;
