//! 认证端点：scrypt 密码 + 随机令牌 + 多设备会话（refresh 单行单次使用轮换）。

use crate::db::QUOTA_BYTES;
use crate::{api_error, data_reply, now_ms, AppState, SharedState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use flowix_sync::{AuthData, CloudSession, CloudUser, MeData, RefreshData};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

const ACCESS_TTL_MS: i64 = 15 * 60 * 1000;
const REFRESH_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// refresh 轮换宽限窗口（§18 遗留①加固）：旧 token 消费后短时间内
/// 允许幂等重放，覆盖「服务端已消费、客户端尚未写回 secrets」的强杀
/// 窗口——否则客户端残留已消费 token，重启 restore 即被判失效登出。
const REFRESH_GRACE_MS: i64 = 60_000;

struct GraceEntry {
    /// 首次轮换的完整 RefreshData（重放返回同一新 token 对，保证幂等）
    reply: serde_json::Value,
    /// 轮换出的新 refresh token hash：验活用——新行仍在主表才允许重放
    /// （logout / 新 token 被再次轮换消费后，旧 token 不能复活会话）
    next_refresh_hash: String,
    expires_at: i64,
}

/// 消费后的 refresh token 宽限缓存（内存，单实例自建云）。
/// 重启丢失可接受：宽限只覆盖 60s 窗口，重启同时再撞窗口的概率忽略不计。
#[derive(Default)]
pub struct RefreshGrace {
    entries: Mutex<HashMap<String, GraceEntry>>,
}

impl RefreshGrace {
    fn record(&self, consumed_hash: &str, reply: serde_json::Value, next_refresh_token: &str) {
        let mut entries = self.entries.lock().unwrap();
        let now = now_ms();
        entries.retain(|_, entry| entry.expires_at > now);
        entries.insert(
            consumed_hash.to_string(),
            GraceEntry {
                reply,
                next_refresh_hash: sha256_hex(next_refresh_token),
                expires_at: now + REFRESH_GRACE_MS,
            },
        );
    }

    /// 宽限重放：命中且未过期且新 refresh 行仍在主表 → 返回首次轮换响应。
    fn replay(
        &self,
        consumed_hash: &str,
        connection: &rusqlite::Connection,
    ) -> Option<serde_json::Value> {
        let now = now_ms();
        let (reply, next_refresh_hash) = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.get(consumed_hash)?;
            if entry.expires_at <= now {
                entries.remove(consumed_hash);
                return None;
            }
            (entry.reply.clone(), entry.next_refresh_hash.clone())
        };
        // 不持锁做 DB 查询；新行不在（logout/已再轮换）→ 宽限失效
        let next_alive: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM refresh_tokens WHERE token_hash = ?1)",
                [&next_refresh_hash],
                |row| row.get(0),
            )
            .unwrap_or(false);
        next_alive.then_some(reply)
    }
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_password(password: &str) -> String {
    let salt = uuid::Uuid::now_v7().simple().to_string();
    let params = scrypt::Params::recommended();
    let mut derived = vec![0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut derived)
        .expect("scrypt derive with recommended params");
    format!("{salt}:{}", hex_encode(&derived))
}

fn verify_password(password: &str, stored: &str) -> bool {
    let Some((salt, hash)) = stored.split_once(':') else {
        return false;
    };
    let expected = match hex_decode(hash) {
        Some(value) if value.len() == 32 => value,
        _ => return false,
    };
    let params = scrypt::Params::recommended();
    let mut derived = vec![0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut derived)
        .is_ok_and(|()| constant_time_eq(&derived, &expected))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn issue_session(connection: &rusqlite::Connection, user_id: &str) -> Result<CloudSession, String> {
    let access_token = format!("at_{}", uuid::Uuid::now_v7().simple());
    let refresh_token = format!("rt_{}", uuid::Uuid::now_v7().simple());
    let now = now_ms();
    // 多设备会话语义：login/refresh 各自持有独立行，互不作废。
    // 单次使用语义由调用方维护（refresh 先删被消费行再发新行）。
    connection
        .execute(
            "INSERT INTO access_tokens (token_hash, user_id, expires_at) VALUES (?1,?2,?3)",
            rusqlite::params![sha256_hex(&access_token), user_id, now + ACCESS_TTL_MS],
        )
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "INSERT INTO refresh_tokens (token_hash, user_id, expires_at, created_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![sha256_hex(&refresh_token), user_id, now + REFRESH_TTL_MS, now],
        )
        .map_err(|e| e.to_string())?;
    Ok(CloudSession {
        access_token_expires_at: now + ACCESS_TTL_MS,
        refresh_token_expires_at: now + REFRESH_TTL_MS,
        access_token,
        refresh_token,
    })
}

fn load_user(connection: &rusqlite::Connection, user_id: &str) -> Option<CloudUser> {
    connection
        .query_row(
            "SELECT id, email, display_name, system_role FROM users WHERE id = ?1",
            [user_id],
            |row| {
                Ok(CloudUser {
                    id: row.get(0)?,
                    email: row.get(1)?,
                    display_name: row.get(2)?,
                    system_role: row.get(3)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsBody {
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
}

pub async fn register(
    State(state): State<SharedState>,
    Json(body): Json<CredentialsBody>,
) -> axum::response::Response {
    let email = body.email.trim().to_ascii_lowercase();
    if !email.contains('@') || !email.contains('.') || email.contains(char::is_whitespace) {
        return api_error(StatusCode::BAD_REQUEST, "INVALID_EMAIL", "邮箱格式不正确");
    }
    if body.password.len() < 8 {
        return api_error(StatusCode::BAD_REQUEST, "WEAK_PASSWORD", "密码至少 8 位");
    }
    let user_id = format!("usr_{}", uuid::Uuid::now_v7().simple());
    let display_name = body
        .display_name
        .clone()
        .unwrap_or_else(|| email.split('@').next().unwrap_or("user").to_string());
    let result = state.db.with_transaction(|connection| {
        let taken: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE email = ?1)",
                [&email],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if taken {
            return Err("EMAIL_TAKEN".to_string());
        }
        connection
            .execute(
                "INSERT INTO users (id, email, password_hash, display_name, system_role, created_at)
                 VALUES (?1,?2,?3,?4,'user',?5)",
                rusqlite::params![
                    user_id,
                    email,
                    hash_password(&body.password),
                    display_name,
                    now_ms()
                ],
            )
            .map_err(|e| e.to_string())?;
        let user = load_user(connection, &user_id).ok_or("user vanished after insert")?;
        let session = issue_session(connection, &user_id)?;
        Ok(AuthData { user, session })
    });
    match result {
        Ok(data) => data_reply(data),
        Err(message) if message == "EMAIL_TAKEN" => {
            api_error(StatusCode::CONFLICT, "EMAIL_TAKEN", "该邮箱已注册")
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

pub async fn login(
    State(state): State<SharedState>,
    Json(body): Json<CredentialsBody>,
) -> axum::response::Response {
    let email = body.email.trim().to_ascii_lowercase();
    let result = state.db.with_transaction(|connection| {
        let row: Option<(String, String)> = connection
            .query_row(
                "SELECT id, password_hash FROM users WHERE email = ?1",
                [&email],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((user_id, password_hash)) = row else {
            return Err("INVALID_CREDENTIALS".to_string());
        };
        if !verify_password(&body.password, &password_hash) {
            return Err("INVALID_CREDENTIALS".to_string());
        }
        let user = load_user(connection, &user_id).ok_or("user vanished")?;
        let session = issue_session(connection, &user_id)?;
        Ok(AuthData { user, session })
    });
    match result {
        Ok(data) => data_reply(data),
        Err(message) if message == "INVALID_CREDENTIALS" => {
            api_error(StatusCode::UNAUTHORIZED, "INVALID_CREDENTIALS", "邮箱或密码不正确")
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshBody {
    pub refresh_token: String,
}

/// refresh 结果：正常轮换出新会话，或宽限窗口内幂等返回首次轮换的响应。
enum RefreshOutcome {
    Rotated(CloudSession),
    Grace(serde_json::Value),
}

pub async fn refresh(
    State(state): State<SharedState>,
    Json(body): Json<RefreshBody>,
) -> axum::response::Response {
    let token_hash = sha256_hex(&body.refresh_token);
    let result = state.db.with_transaction(|connection| {
        let row: Option<(String, i64)> = connection
            .query_row(
                "SELECT user_id, expires_at FROM refresh_tokens WHERE token_hash = ?1",
                [&token_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((user_id, expires_at)) = row else {
            // §18 遗留①：主表 miss 先查宽限——旧 token 在消费后短窗口内的
            // 重放（客户端强杀落轮换窗口后的恢复）幂等返回同一新 token 对
            if let Some(reply) = state.refresh_grace.replay(&token_hash, connection) {
                return Ok(RefreshOutcome::Grace(reply));
            }
            return Err("INVALID_REFRESH_TOKEN".to_string());
        };
        if expires_at < now_ms() {
            let _ = connection.execute(
                "DELETE FROM refresh_tokens WHERE token_hash = ?1",
                [&token_hash],
            );
            return Err("INVALID_REFRESH_TOKEN".to_string());
        }
        // 单次使用：只作废被消费的这一行，其它设备的会话不受影响
        connection
            .execute(
                "DELETE FROM refresh_tokens WHERE token_hash = ?1",
                [&token_hash],
            )
            .map_err(|e| e.to_string())?;
        let session = issue_session(connection, &user_id)?;
        // 消费后登记宽限（重放返回与本次完全相同的响应）
        let reply = serde_json::to_value(RefreshData {
            session: session.clone(),
        })
        .map_err(|e| e.to_string())?;
        state
            .refresh_grace
            .record(&token_hash, reply, &session.refresh_token);
        Ok(RefreshOutcome::Rotated(session))
    });
    match result {
        // 宽限响应已是 RefreshData 的 JSON 形态，data_reply 包同一信封
        Ok(RefreshOutcome::Grace(reply)) => data_reply(reply),
        Ok(RefreshOutcome::Rotated(session)) => data_reply(RefreshData { session }),
        Err(message) if message == "INVALID_REFRESH_TOKEN" => {
            api_error(StatusCode::UNAUTHORIZED, "INVALID_REFRESH_TOKEN", "会话已过期，请重新登录")
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

pub async fn logout_refresh(
    State(state): State<SharedState>,
    Json(body): Json<RefreshBody>,
) -> axum::response::Response {
    let token_hash = sha256_hex(&body.refresh_token);
    let _ = state.db.with(|connection| {
        connection
            .execute(
                "DELETE FROM refresh_tokens WHERE token_hash = ?1",
                [&token_hash],
            )
            .map_err(|e| e.to_string())
    });
    data_reply(serde_json::json!({}))
}

/// Bearer 访问令牌 → user_id。所有 /v2 端点用它鉴权。
pub fn require_user(state: &AppState, headers: &HeaderMap) -> Result<String, axum::response::Response> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token else {
        return Err(api_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "缺少访问令牌"));
    };
    let token_hash = sha256_hex(token);
    let user_id: Option<(String, i64)> = state
        .db
        .with(|connection| {
            connection
                .query_row(
                    "SELECT user_id, expires_at FROM access_tokens WHERE token_hash = ?1",
                    [&token_hash],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())
        })
        .ok()
        .flatten();
    match user_id {
        Some((user_id, expires_at)) if expires_at >= now_ms() => Ok(user_id),
        _ => Err(api_error(
            StatusCode::UNAUTHORIZED,
            "INVALID_TOKEN",
            "访问令牌无效或已过期",
        )),
    }
}

pub async fn me(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let user_id = match require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let user = state
        .db
        .with(|connection| Ok::<_, String>(load_user(connection, &user_id)))
        .ok()
        .flatten();
    match user {
        Some(user) => data_reply(MeData { user }),
        None => api_error(StatusCode::UNAUTHORIZED, "INVALID_TOKEN", "账号不存在"),
    }
}

/// 自建版权益：永久免费 100GB。
pub async fn entitlements(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let user_id = match require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (used_bytes, note_count) = state
        .db
        .with(|connection| Ok(crate::db::user_usage(connection, &user_id)))
        .unwrap_or((0, 0));
    data_reply(serde_json::json!({
        "membership": { "active": true, "startsAt": null, "expiresAt": null },
        "usage": {
            "usedBytes": used_bytes,
            "quotaBytes": QUOTA_BYTES,
            "availableBytes": QUOTA_BYTES - used_bytes,
            "noteCount": note_count,
            "readOnly": false,
        }
    }))
}

pub async fn not_supported() -> axum::response::Response {
    api_error(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_SUPPORTED",
        "自建版暂不支持 Apple/Google 登录",
    )
}
