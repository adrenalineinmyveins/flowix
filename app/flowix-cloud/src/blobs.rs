//! blob 代理存储：内容寻址（SHA-256 base64url 无填充，复用 `v2_content_hash`）。
//! 代理模式：预约返回 upload.path，上传即完成（无 completionPath）。

use crate::{api_error, data_reply, now_ms, raw_reply, SharedState};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use flowix_sync::{
    v2_content_hash, V2BlobDownload, V2BlobDownloadCapability, V2BlobDownloadEnvelope,
    V2BlobReservation, V2BlobReservationEnvelope, V2BlobUpload,
};
use rusqlite::OptionalExtension;
use serde::Deserialize;

const RESERVATION_TTL_MS: i64 = 30 * 60 * 1000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReserveBody {
    pub protocol_epoch: i64,
    pub content_hash: String,
    pub size_bytes: i64,
    pub blob_kind: Option<String>,
    pub content_type: Option<String>,
}

pub async fn reserve(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ReserveBody>,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if body.protocol_epoch != flowix_sync::PROTOCOL_EPOCH {
        return api_error(StatusCode::BAD_REQUEST, "PROTOCOL_MISMATCH", "仅支持协议 epoch 2");
    }
    if body.content_hash.is_empty() || body.size_bytes < 0 {
        return api_error(StatusCode::BAD_REQUEST, "INVALID_RESERVATION", "预约参数不完整");
    }
    let id = format!("res_{}", uuid::Uuid::now_v7().simple());
    let expires_at = now_ms() + RESERVATION_TTL_MS;
    if let Err(error) = state.db.with(|connection| {
        connection
            .execute(
                "INSERT INTO blob_reservations (id, user_id, content_hash, size_bytes, blob_kind, content_type, expires_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![
                    id,
                    user_id,
                    body.content_hash,
                    body.size_bytes,
                    body.blob_kind.as_deref().unwrap_or("unknown"),
                    body.content_type.as_deref().unwrap_or("application/octet-stream"),
                    expires_at
                ],
            )
            .map_err(|e| e.to_string())
    }) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error);
    }
    raw_reply(V2BlobReservationEnvelope {
        data: V2BlobReservation {
            reservation_id: id.clone(),
            content_hash: body.content_hash,
            size_bytes: body.size_bytes,
            expires_at,
        },
        upload: V2BlobUpload {
            method: "PUT".into(),
            path: Some(format!("/v2/blobs/reservations/{id}")),
            url: None,
            headers: Default::default(),
            expires_at: Some(expires_at),
            completion_path: None,
        },
    })
}

pub async fn upload(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Path(reservation_id): Path<String>,
    bytes: Bytes,
) -> axum::response::Response {
    let user_id = match crate::auth::require_user(&state, &headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let result = state.db.with_transaction(|connection| {
        let reservation = connection
            .query_row(
                "SELECT user_id, content_hash, size_bytes, blob_kind, content_type, expires_at
                 FROM blob_reservations WHERE id = ?1",
                [&reservation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((owner, content_hash, size_bytes, blob_kind, content_type, expires_at)) =
            reservation
        else {
            return Err("RESERVATION_NOT_FOUND".to_string());
        };
        if owner != user_id || expires_at < now_ms() {
            return Err("RESERVATION_NOT_FOUND".to_string());
        }
        if bytes.len() as i64 != size_bytes {
            return Err(format!(
                "SIZE_MISMATCH: 预约 {size_bytes} 字节，实际 {} 字节",
                bytes.len()
            ));
        }
        let actual = v2_content_hash(&bytes);
        if actual != content_hash {
            return Err("HASH_MISMATCH: 内容哈希与预约不符".to_string());
        }
        connection
            .execute(
                "INSERT OR IGNORE INTO blobs (content_hash, size_bytes, content_type, blob_kind, data, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![
                    content_hash,
                    size_bytes,
                    content_type,
                    blob_kind,
                    &bytes[..],
                    now_ms()
                ],
            )
            .map_err(|e| e.to_string())?;
        connection
            .execute("DELETE FROM blob_reservations WHERE id = ?1", [&reservation_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    });
    match result {
        Ok(()) => data_reply(serde_json::json!({ "stored": true })),
        Err(message) if message == "RESERVATION_NOT_FOUND" => {
            api_error(StatusCode::NOT_FOUND, "RESERVATION_NOT_FOUND", "预约不存在或已过期")
        }
        Err(message) if message.starts_with("SIZE_MISMATCH") => {
            api_error(StatusCode::BAD_REQUEST, "SIZE_MISMATCH", message)
        }
        Err(message) if message.starts_with("HASH_MISMATCH") => {
            api_error(StatusCode::BAD_REQUEST, "HASH_MISMATCH", message)
        }
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", error),
    }
}

pub async fn complete(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Path(_reservation_id): Path<String>,
) -> axum::response::Response {
    if let Err(response) = crate::auth::require_user(&state, &headers) {
        return response;
    }
    // 代理模式上传即完成；端点保留以满足客户端 completionPath 约定
    data_reply(serde_json::json!({}))
}

pub async fn access(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Path(content_hash): Path<String>,
) -> axum::response::Response {
    if let Err(response) = crate::auth::require_user(&state, &headers) {
        return response;
    }
    let exists: bool = state
        .db
        .with(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM blobs WHERE content_hash = ?1)",
                    [&content_hash],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())
        })
        .unwrap_or(false);
    if !exists {
        return api_error(StatusCode::NOT_FOUND, "BLOB_NOT_FOUND", "内容不存在");
    }
    raw_reply(V2BlobDownloadEnvelope {
        data: V2BlobDownload {
            content_hash: content_hash.clone(),
        },
        download: V2BlobDownloadCapability {
            method: "GET".into(),
            path: Some(format!("/v2/blobs/{content_hash}")),
            url: None,
            headers: Default::default(),
            expires_at: None,
        },
    })
}

pub async fn download(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Path(content_hash): Path<String>,
) -> axum::response::Response {
    if let Err(response) = crate::auth::require_user(&state, &headers) {
        return response;
    }
    let blob: Option<(Vec<u8>, String)> = state
        .db
        .with(|connection| {
            connection
                .query_row(
                    "SELECT data, content_type FROM blobs WHERE content_hash = ?1",
                    [&content_hash],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())
        })
        .ok()
        .flatten();
    match blob {
        Some((data, content_type)) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_str(&content_type)
                    .unwrap_or(axum::http::HeaderValue::from_static("application/octet-stream")),
            )],
            data,
        )
            .into_response(),
        None => api_error(StatusCode::NOT_FOUND, "BLOB_NOT_FOUND", "内容不存在"),
    }
}
