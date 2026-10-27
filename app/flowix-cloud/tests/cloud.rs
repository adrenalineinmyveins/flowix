//! 全协议集成测试：真实 HTTP 回路（对应执行方案 §9 的冒烟清单）。
//! 覆盖：注册/登录/refresh 轮换、blob 预约-上传-下载、push（含 create-or-adopt
//! 与 REVISION_CONFLICT 冲突语义）、changes 游标、bootstrap 快照、history。

use axum::body::Body;
use flowix_cloud::db::CloudDb;
use flowix_cloud::AppState;
use flowix_sync::{
    v2_content_hash, ApiErrorEnvelope, AuthData, DataEnvelope, V2Bootstrap, V2Change,
    V2ChangesPage, V2History, V2PushResult, V2SyncStatus,
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

async fn serve() -> (axum::Router, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let db = CloudDb::open(&temp.path().join("cloud.db")).unwrap();
    (
        flowix_cloud::build_router(Arc::new(AppState {
            db,
            refresh_grace: Default::default(),
        })),
        temp,
    )
}

async fn request_json<T: serde::de::DeserializeOwned>(
    router: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, Result<T, String>) {
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("{error}: {}", String::from_utf8_lossy(&bytes))),
    )
}

#[tokio::test]
async fn full_protocol_roundtrip() {
    let (router, _temp) = serve().await;
    let email = format!("cloud{}@test.local", uuid::Uuid::now_v7().simple());

    // 1. 注册 + 登录态解析
    let (status, auth) = request_json::<DataEnvelope<AuthData>>(
        &router,
        "POST",
        "/v1/auth/register",
        None,
        Some(serde_json::json!({ "email": email, "password": "CloudTest2026", "displayName": "Cloud" })),
    )
    .await;
    assert_eq!(status, 200, "register failed: {auth:?}");
    let auth = auth.unwrap().data;
    assert_eq!(auth.user.email, email);

    // 2. refresh 轮换 + 宽限重放（§18 遗留①加固）：
    //    首次轮换旧→新A；旧 token 在宽限窗口内重放 → 200 且返回同一新A（幂等）；
    //    新A 被再次轮换消费（→新B）后，旧 token 重放 → 401（验活失效，不能复活）
    let (status, refreshed) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router,
        "POST",
        "/v1/auth/refresh",
        None,
        Some(serde_json::json!({ "refreshToken": auth.session.refresh_token })),
    )
    .await;
    assert_eq!(status, 200);
    let rotated = refreshed.unwrap().data.session;
    let (status, replayed) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router,
        "POST",
        "/v1/auth/refresh",
        None,
        Some(serde_json::json!({ "refreshToken": auth.session.refresh_token })),
    )
    .await;
    assert_eq!(status, 200, "宽限窗口内旧 token 重放应幂等成功");
    let replayed = replayed.unwrap().data.session;
    assert_eq!(
        replayed.refresh_token, rotated.refresh_token,
        "宽限重放必须返回同一新 refresh token"
    );
    assert_eq!(replayed.access_token, rotated.access_token);
    // 用宽限返回的新A 继续轮换 → 新B（正常链路）
    let (status, next) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router,
        "POST",
        "/v1/auth/refresh",
        None,
        Some(serde_json::json!({ "refreshToken": rotated.refresh_token })),
    )
    .await;
    assert_eq!(status, 200);
    let next = next.unwrap().data.session;
    // 新A 已消费 → 旧 token 宽限验活失败 → 401
    let (status, _) = request_json::<DataEnvelope<AuthData>>(
        &router,
        "POST",
        "/v1/auth/refresh",
        None,
        Some(serde_json::json!({ "refreshToken": auth.session.refresh_token })),
    )
    .await;
    assert_eq!(status, 401, "新 token 已被再次轮换后，旧 token 宽限应失效");
    // 新A 也已消费（→新B）→ 宽限窗口内重放返回新B（每次轮换的宽限同构）
    let (status, replay_a) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router,
        "POST",
        "/v1/auth/refresh",
        None,
        Some(serde_json::json!({ "refreshToken": rotated.refresh_token })),
    )
    .await;
    assert_eq!(status, 200, "新A 消费后宽限窗口内重放应幂等成功");
    assert_eq!(
        replay_a.unwrap().data.session.refresh_token,
        next.refresh_token,
        "新A 重放返回新B（同一 token 对）"
    );
    let token = next.access_token;

    // 3. blob 预约 → 上传 → 下载
    let content = "# 云端契约测试\n\n正文段落。\n".as_bytes().to_vec();
    let hash = v2_content_hash(&content);
    let (status, reservation) = request_json::<flowix_sync::V2BlobReservationEnvelope>(
        &router,
        "POST",
        "/v2/blobs/reservations",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "contentHash": hash,
            "sizeBytes": content.len(),
            "blobKind": "note",
            "contentType": "text/markdown",
        })),
    )
    .await;
    assert_eq!(status, 200, "reserve failed");
    let envelope = reservation.unwrap();
    let upload_path = envelope.upload.path.clone().unwrap();
    assert!(upload_path.starts_with("/v2/blobs/reservations/"));

    let upload_request = axum::http::Request::builder()
        .method("PUT")
        .uri(&upload_path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "text/markdown")
        .body(Body::from(content.clone()))
        .unwrap();
    let response = router.clone().oneshot(upload_request).await.unwrap();
    assert_eq!(response.status(), 200);

    let download_request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/v2/blobs/{hash}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(download_request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], &content[..], "blob 往返应逐字节一致");

    // 4. push：notebook + note（null base 新建）
    let note_id = flowix_sync::v2_path_note_id("nb_it", "契约测试.md");
    let push_body = serde_json::json!({
        "protocolEpoch": 2,
        "operations": [
            { "type": "notebook.put", "operationId": "op_nb_1", "baseRevision": null,
              "notebook": { "id": "nb_it", "name": "集成测试本", "icon": null, "sortOrder": 0 } },
            { "type": "note.put", "operationId": "op_note_1", "baseRevision": null,
              "note": { "id": note_id, "notebookId": "nb_it", "filename": "契约测试.md",
                        "fileKind": "markdown", "contentHash": hash,
                        "sizeBytes": content.len(), "attachments": [] } },
        ],
    });
    let (status, push) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(push_body),
    )
    .await;
    assert_eq!(status, 200);
    let push = push.unwrap().data;
    assert!(push.results.iter().all(|result| result.ok), "{:?}", push.results);
    let note_revision = push.results[1]
        .data
        .as_ref()
        .expect("note put result data")
        .revision
        .clone();

    // 5. notebook adopt：base=null 再推已存在笔记本 → 接受（移动端认领语义）
    let (status, adopt) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "notebook.put", "operationId": "op_nb_adopt", "baseRevision": null,
                  "notebook": { "id": "nb_it", "name": "集成测试本", "icon": null, "sortOrder": 0 } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        adopt.unwrap().data.results[0].ok,
        "notebook create-or-adopt 应接受 null base"
    );

    // 6. 冲突：错误 base 推 note → 操作级 409 + currentRevision（三方合并契约）
    let (status, conflict) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "note.put", "operationId": "op_note_conflict", "baseRevision": "rev_wrong",
                  "note": { "id": note_id, "notebookId": "nb_it", "filename": "契约测试.md",
                            "fileKind": "markdown", "contentHash": hash,
                            "sizeBytes": content.len(), "attachments": [] } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200, "操作级冲突在 200 响应体内");
    let conflict_result = conflict.unwrap().data.results.remove(0);
    assert!(!conflict_result.ok);
    assert_eq!(conflict_result.status, 409);
    let error = conflict_result.error.expect("conflict error");
    assert_eq!(error.code, "REVISION_CONFLICT");
    assert_eq!(
        error.details.expect("conflict details")["currentRevision"],
        serde_json::json!(note_revision)
    );

    // 7. 正确 base 重推 → replacedRevision 指向旧版本
    let (status, rebase) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "note.put", "operationId": "op_note_2", "baseRevision": note_revision,
                  "note": { "id": note_id, "notebookId": "nb_it", "filename": "契约测试.md",
                            "fileKind": "markdown", "contentHash": hash,
                            "sizeBytes": content.len(), "attachments": [] } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let rebase_result = rebase.unwrap().data.results.remove(0);
    assert!(rebase_result.ok);
    assert_eq!(
        rebase_result.data.as_ref().unwrap().replaced_revision.as_deref(),
        Some(note_revision.as_str())
    );

    // 8. changes 游标增量
    let (status, changes) = request_json::<DataEnvelope<V2ChangesPage>>(
        &router,
        "GET",
        "/v2/sync/changes?cursor=0&limit=50",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let page = changes.unwrap().data;
    assert!(page.changes.len() >= 4, "应有 notebook×2 + note×2 条变更");
    assert!(!page.has_more);
    let entity_kinds: Vec<String> = page
        .changes
        .iter()
        .map(|change: &V2Change| format!("{}/{}", change.entity_type, change.kind))
        .collect();
    assert!(entity_kinds.contains(&"notebook/put".to_string()));
    assert!(entity_kinds.contains(&"note/put".to_string()));

    // 9. bootstrap 快照（含 syncSeq 与 usage）
    let (status, bootstrap) = request_json::<DataEnvelope<V2Bootstrap>>(
        &router,
        "GET",
        "/v2/sync/bootstrap?limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let snapshot = bootstrap.unwrap().data;
    assert_eq!(snapshot.notebooks.len(), 1);
    assert_eq!(snapshot.notes.len(), 1);
    assert!(snapshot.notes[0].sync_seq > 0, "syncSeq 必填且非零");
    assert_eq!(snapshot.notes[0].content_hash.as_deref(), Some(hash.as_str()));
    assert!(snapshot.usage.quota_bytes > 0);

    // 10. history + status
    let (status, history) = request_json::<DataEnvelope<V2History>>(
        &router,
        "GET",
        &format!("/v2/sync/history?noteId={note_id}&limit=10"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let history = history.unwrap().data;
    assert!(history.revisions.len() >= 2, "应有 ≥2 个版本（put×2）");

    let (status, status_data) = request_json::<DataEnvelope<V2SyncStatus>>(
        &router,
        "GET",
        "/v2/sync/status?cursor=0",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let status_data = status_data.unwrap().data;
    assert!(status_data.has_changes);
    assert!(status_data.head_cursor >= page.head_cursor);
}

#[tokio::test]
async fn error_envelope_and_auth_boundaries() {
    let (router, _temp) = serve().await;

    // 未带令牌访问受保护端点 → 401 错误信封
    let (status, error) = request_json::<ApiErrorEnvelope>(
        &router,
        "GET",
        "/v2/sync/status?cursor=0",
        None,
        None,
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(error.unwrap().error.code, "UNAUTHORIZED");

    // 弱密码注册 → 400
    let (status, _) = request_json::<ApiErrorEnvelope>(
        &router,
        "POST",
        "/v1/auth/register",
        None,
        Some(serde_json::json!({ "email": "weak@test.local", "password": "short" })),
    )
    .await;
    assert_eq!(status, 400);

    // 重复注册 → 409 EMAIL_TAKEN
    let body = serde_json::json!({ "email": "dup@test.local", "password": "CloudTest2026" });
    let (status, _) = request_json::<ApiErrorEnvelope>(
        &router,
        "POST",
        "/v1/auth/register",
        None,
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 200);
    let (status, taken) = request_json::<ApiErrorEnvelope>(
        &router,
        "POST",
        "/v1/auth/register",
        None,
        Some(body),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(taken.unwrap().error.code, "EMAIL_TAKEN");

    // 错误密码登录 → 401
    let (status, _) = request_json::<ApiErrorEnvelope>(
        &router,
        "POST",
        "/v1/auth/login",
        None,
        Some(serde_json::json!({ "email": "dup@test.local", "password": "WrongPassword1" })),
    )
    .await;
    assert_eq!(status, 401);

    // Apple 登录 → 501（自建版不支持）
    let (status, _) = request_json::<ApiErrorEnvelope>(
        &router,
        "POST",
        "/v1/auth/apple/exchange",
        None,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 501);
}

/// 多设备会话共存：两次 login 各持独立 refresh token；A 轮换不影响 B；
/// logout 只下线自己；被消费 token 单次使用失效语义保留。
#[tokio::test]
async fn multi_device_sessions_coexist() {
    let (router, _temp) = serve().await;
    let email = format!("multi{}@test.local", uuid::Uuid::now_v7().simple());
    let creds = serde_json::json!({ "email": email, "password": "CloudTest2026", "displayName": "M" });
    let (status, _) = request_json::<DataEnvelope<AuthData>>(
        &router, "POST", "/v1/auth/register", None, Some(creds.clone()),
    ).await;
    assert_eq!(status, 200);

    // 设备 A、B 分别 login（旧语义下 B 的 login 会作废 A 的 token）
    let login_body = serde_json::json!({ "email": email, "password": "CloudTest2026" });
    let (_, a) = request_json::<DataEnvelope<AuthData>>(
        &router, "POST", "/v1/auth/login", None, Some(login_body.clone()),
    ).await;
    let a = a.unwrap().data.session;
    let (_, b) = request_json::<DataEnvelope<AuthData>>(
        &router, "POST", "/v1/auth/login", None, Some(login_body),
    ).await;
    let b = b.unwrap().data.session;

    // A refresh 成功后，B 仍可 refresh（共存）；B 轮换后的新 token 留作后续断言
    let (status, a2) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router, "POST", "/v1/auth/refresh", None,
        Some(serde_json::json!({ "refreshToken": a.refresh_token })),
    ).await;
    assert_eq!(status, 200, "A refresh 应成功");
    let a2 = a2.unwrap().data.session;
    let (status, b2) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router, "POST", "/v1/auth/refresh", None,
        Some(serde_json::json!({ "refreshToken": b.refresh_token })),
    ).await;
    assert_eq!(status, 200, "A 轮换后 B 不应被踢（多设备共存）");
    let b2 = b2.unwrap().data.session;

    // 单次使用 + 宽限：A 旧 token 已随自己那次 refresh 作废；60s 宽限窗口内
    // 重放幂等返回与 A 轮换相同的新 token 对（§18 遗留①加固语义）
    let (status, a_replay) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router, "POST", "/v1/auth/refresh", None,
        Some(serde_json::json!({ "refreshToken": a.refresh_token })),
    ).await;
    assert_eq!(status, 200, "宽限窗口内 A 旧 token 重放应幂等成功");
    assert_eq!(
        a_replay.unwrap().data.session.refresh_token,
        a2.refresh_token,
        "A 旧 token 重放返回其轮换出的同一新 token"
    );

    // logout 只下线自己：A logout → A 失效（宽限随之失效）→ B 仍有效
    let (status, _) = request_json::<serde_json::Value>(
        &router, "POST", "/v1/auth/logout/refresh", None,
        Some(serde_json::json!({ "refreshToken": a2.refresh_token })),
    ).await;
    assert_eq!(status, 200);
    let (status, _) = request_json::<ApiErrorEnvelope>(
        &router, "POST", "/v1/auth/refresh", None,
        Some(serde_json::json!({ "refreshToken": a2.refresh_token })),
    ).await;
    assert_eq!(status, 401, "A logout 后自己的 token 应失效");
    // logout 后 A 旧 token 的宽限验活失败（新行 a2 已删）→ 不能复活
    let (status, _) = request_json::<ApiErrorEnvelope>(
        &router, "POST", "/v1/auth/refresh", None,
        Some(serde_json::json!({ "refreshToken": a.refresh_token })),
    ).await;
    assert_eq!(status, 401, "A logout 后旧 token 宽限应失效（不能复活会话）");
    let (status, _) = request_json::<DataEnvelope<flowix_sync::RefreshData>>(
        &router, "POST", "/v1/auth/refresh", None,
        Some(serde_json::json!({ "refreshToken": b2.refresh_token })),
    ).await;
    assert_eq!(status, 200, "A logout 不应影响 B");
}

/// 方案 A thread 实体全链路：blob → put 新建 → 409 冲突 → base 重推 →
/// bootstrap/changes 可见 → th_ history → delete。
#[tokio::test]
async fn thread_entity_roundtrip() {
    let (router, _temp) = serve().await;
    let email = format!("thread{}@test.local", uuid::Uuid::now_v7().simple());
    let (status, auth) = request_json::<DataEnvelope<AuthData>>(
        &router,
        "POST",
        "/v1/auth/register",
        None,
        Some(serde_json::json!({ "email": email, "password": "CloudTest2026", "displayName": "T" })),
    )
    .await;
    assert_eq!(status, 200, "register failed: {auth:?}");
    let token = auth.unwrap().data.session.access_token;

    // 1. thread 消息体 blob 上传（整 Thread 单 JSON）
    let thread_id = format!("th_{}", uuid::Uuid::now_v7().simple());
    let content = format!(r#"{{"id":"{thread_id}","title":"移动端会话","messages":[]}}"#);
    let content = content.into_bytes();
    let hash = v2_content_hash(&content);
    let (status, reservation) = request_json::<flowix_sync::V2BlobReservationEnvelope>(
        &router,
        "POST",
        "/v2/blobs/reservations",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "contentHash": hash,
            "sizeBytes": content.len(),
            "blobKind": "thread",
            "contentType": "application/json",
        })),
    )
    .await;
    assert_eq!(status, 200, "reserve failed");
    let upload_path = reservation.unwrap().upload.path.unwrap();
    let upload_request = axum::http::Request::builder()
        .method("PUT")
        .uri(&upload_path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(content.clone()))
        .unwrap();
    let response = router.clone().oneshot(upload_request).await.unwrap();
    assert_eq!(response.status(), 200);

    // 2. thread.put 新建（null base）
    let (status, push) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_th_1", "baseRevision": null,
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "移动端会话",
                              "contentHash": hash, "sizeBytes": content.len() } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let push = push.unwrap().data;
    assert!(push.results[0].ok, "{:?}", push.results);
    let put_data = push.results[0].data.as_ref().unwrap();
    assert_eq!(put_data.entity_type, "thread");
    assert_eq!(put_data.resolution, "created");
    let thread_revision = put_data.revision.clone();

    // 3. blob 未上传先 put → BLOB_MISSING
    let (status, missing) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_th_missing", "baseRevision": thread_revision,
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "移动端会话",
                              "contentHash": v2_content_hash(b"never uploaded"),
                              "sizeBytes": 14 } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let missing_result = missing.unwrap().data.results.remove(0);
    assert!(!missing_result.ok);
    assert_eq!(missing_result.error.unwrap().code, "BLOB_MISSING");

    // 4. 错误 base → 操作级 409 + currentRevision（消息级合并契约）
    let (status, conflict) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_th_conflict", "baseRevision": "rev_wrong",
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "并发会话",
                              "contentHash": hash, "sizeBytes": content.len() } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let conflict_result = conflict.unwrap().data.results.remove(0);
    assert!(!conflict_result.ok);
    assert_eq!(conflict_result.status, 409);
    let error = conflict_result.error.expect("conflict error");
    assert_eq!(error.code, "REVISION_CONFLICT");
    assert_eq!(
        error.details.unwrap()["currentRevision"],
        serde_json::json!(thread_revision)
    );

    // 5. 正确 base 重推（标题更新）→ replacedRevision
    let (status, rebase) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_th_2", "baseRevision": thread_revision,
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "合并后会话",
                              "contentHash": hash, "sizeBytes": content.len() } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let rebase_result = rebase.unwrap().data.results.remove(0);
    assert!(rebase_result.ok, "rebase push should succeed");
    let updated_revision = rebase_result.data.as_ref().unwrap().revision.clone();
    assert_eq!(
        rebase_result.data.unwrap().replaced_revision.as_deref(),
        Some(thread_revision.as_str())
    );

    // 6. changes 流：thread/put 出现且携带 agentId
    let (status, changes) = request_json::<DataEnvelope<V2ChangesPage>>(
        &router,
        "GET",
        "/v2/sync/changes?cursor=0&limit=50",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let page = changes.unwrap().data;
    let thread_puts: Vec<&V2Change> = page
        .changes
        .iter()
        .filter(|change| change.entity_type == "thread" && change.kind == "put")
        .collect();
    assert_eq!(thread_puts.len(), 2, "{:?}", page.changes);
    assert_eq!(thread_puts[0].agent_id.as_deref(), Some("agent_loop"));
    assert_eq!(thread_puts[0].name.as_deref(), Some("移动端会话"));

    // 7. bootstrap：threads 数组输出
    let (status, bootstrap) = request_json::<DataEnvelope<V2Bootstrap>>(
        &router,
        "GET",
        "/v2/sync/bootstrap?limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let snapshot = bootstrap.unwrap().data;
    assert_eq!(snapshot.threads.len(), 1);
    let thread = &snapshot.threads[0];
    assert_eq!(thread.id, thread_id);
    assert_eq!(thread.agent_id, "agent_loop");
    assert_eq!(thread.title, "合并后会话");
    assert_eq!(thread.content_hash.as_deref(), Some(hash.as_str()));
    assert!(thread.sync_seq > 0);

    // 8. history：th_ 前缀路由到 thread_revisions（title 走 relativePath）
    let (status, history) = request_json::<DataEnvelope<V2History>>(
        &router,
        "GET",
        &format!("/v2/sync/history?noteId={thread_id}&limit=10"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let history = history.unwrap().data;
    assert_eq!(history.current_revision, updated_revision);
    assert!(history.revisions.len() >= 2, "put×2 应有 ≥2 版本");
    assert!(history.revisions.iter().all(|revision| !revision.deleted));

    // 9. thread.delete → ok；重复 delete → revision 已轮换 → 409（与 note 语义一致）
    let (status, deleted) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.delete", "operationId": "op_th_del", "baseRevision": updated_revision,
                  "threadId": thread_id },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let delete_result = deleted.unwrap().data.results.remove(0);
    assert!(delete_result.ok, "{delete_result:?}");
    assert!(delete_result.data.as_ref().unwrap().deleted);

    let (status, gone) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.delete", "operationId": "op_th_del2", "baseRevision": updated_revision,
                  "threadId": thread_id },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200, "操作级错误在 200 体内");
    let gone_result = gone.unwrap().data.results.remove(0);
    assert!(!gone_result.ok);
    assert_eq!(gone_result.error.unwrap().code, "REVISION_CONFLICT");

    // 10. delete 后 bootstrap 标记 deleted，history 含删除版本
    let (status, bootstrap) = request_json::<DataEnvelope<V2Bootstrap>>(
        &router,
        "GET",
        "/v2/sync/bootstrap?limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let snapshot = bootstrap.unwrap().data;
    assert_eq!(snapshot.threads.len(), 1);
    assert!(snapshot.threads[0].deleted, "删除后应标记 deleted 而非消失");
}

/// A2 验收：双模拟客户端并发对话 → 409 → bootstrap+merge → rebase 重推 →
/// 双端拉到消息并集零丢失（方案 A §6.5 冲突消解协议闭环）。
#[tokio::test]
async fn thread_concurrent_merge_zero_loss() {
    let (router, _temp) = serve().await;
    let email = format!("merge{}@test.local", uuid::Uuid::now_v7().simple());
    let (status, auth) = request_json::<DataEnvelope<AuthData>>(
        &router,
        "POST",
        "/v1/auth/register",
        None,
        Some(serde_json::json!({ "email": email, "password": "CloudTest2026", "displayName": "M" })),
    )
    .await;
    assert_eq!(status, 200, "register failed: {auth:?}");
    let token = auth.unwrap().data.session.access_token;
    let thread_id = format!("th_{}", uuid::Uuid::now_v7().simple());

    // 两个 payload 版本：共享 msg_1，各自并发新增一条
    let payload = |title: &str, extra: &str, created: i64| {
        serde_json::json!({
            "version": 1,
            "threadId": thread_id,
            "agentId": "agent_loop",
            "title": title,
            "createdAt": 1,
            "messages": [
                { "id": "msg_1", "role": "user", "content": "共同第一问", "createdAt": 100 },
                { "id": extra, "role": "assistant", "content": "并发回答", "createdAt": created }
            ]
        })
        .to_string()
        .into_bytes()
    };
    let desktop = payload("桌面会话", "msg_desktop", 300);
    let mobile = payload("移动会话", "msg_mobile", 200);

    let upload = |bytes: Vec<u8>| {
        let router = router.clone();
        let token = token.clone();
        async move {
            let hash = v2_content_hash(&bytes);
            let (status, reservation) = request_json::<flowix_sync::V2BlobReservationEnvelope>(
                &router,
                "POST",
                "/v2/blobs/reservations",
                Some(&token),
                Some(serde_json::json!({
                    "protocolEpoch": 2, "contentHash": hash,
                    "sizeBytes": bytes.len(), "blobKind": "thread", "contentType": "application/json",
                })),
            )
            .await;
            assert_eq!(status, 200);
            let upload_path = reservation.unwrap().upload.path.unwrap();
            let request = axum::http::Request::builder()
                .method("PUT")
                .uri(&upload_path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(bytes))
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), 200);
            hash
        }
    };

    // 客户端 A（桌面）先推成功
    let desktop_hash = upload(desktop.clone()).await;
    let (status, push_a) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_desktop", "baseRevision": null,
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "桌面会话",
                              "contentHash": desktop_hash, "sizeBytes": desktop.len() } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let push_a = push_a.unwrap().data;
    assert!(push_a.results[0].ok);
    let base_revision = push_a.results[0].data.as_ref().unwrap().revision.clone();

    // 客户端 B（移动）以过期 base（null=新建）推 → 409
    let mobile_hash = upload(mobile.clone()).await;
    let (status, push_b) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_mobile", "baseRevision": null,
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "移动会话",
                              "contentHash": mobile_hash, "sizeBytes": mobile.len() } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let conflict = push_b.unwrap().data.results.remove(0);
    assert!(!conflict.ok);
    assert_eq!(conflict.error.as_ref().unwrap().code, "REVISION_CONFLICT");
    let current_revision = conflict.error.unwrap().details.unwrap()["currentRevision"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(current_revision, base_revision);

    // B 的引擎消解路径：拉 bootstrap 取远端头 → 下载 blob → 消息级合并
    let (status, bootstrap) = request_json::<DataEnvelope<V2Bootstrap>>(
        &router,
        "GET",
        "/v2/sync/bootstrap?limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let snapshot = bootstrap.unwrap().data;
    let remote_head = snapshot
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .expect("remote head in bootstrap");
    assert_eq!(remote_head.revision, current_revision);
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/v2/blobs/{}", remote_head.content_hash.as_deref().unwrap()))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let remote_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let remote_payload: flowix_sync::V2ThreadPayload =
        serde_json::from_slice(&remote_bytes).unwrap();
    let local_payload: flowix_sync::V2ThreadPayload =
        serde_json::from_slice(&mobile).unwrap();
    let merged = flowix_sync::v2_merge_thread_payloads(&local_payload, &remote_payload).unwrap();
    let merged_ids: Vec<&str> = merged.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        merged_ids,
        vec!["msg_1", "msg_mobile", "msg_desktop"],
        "并集 + (createdAt,id) 序：零丢失"
    );

    // 合并结果重传 blob 并以 currentRevision 为 base rebase 重推 → 成功
    let merged_bytes = serde_json::to_vec(&merged).unwrap();
    let merged_hash = upload(merged_bytes).await;
    let (status, rebase) = request_json::<DataEnvelope<V2PushResult>>(
        &router,
        "POST",
        "/v2/sync/push",
        Some(&token),
        Some(serde_json::json!({
            "protocolEpoch": 2,
            "operations": [
                { "type": "thread.put", "operationId": "op_mobile_merged", "baseRevision": current_revision,
                  "thread": { "id": thread_id, "agentId": "agent_loop", "title": "移动会话",
                              "contentHash": merged_hash, "sizeBytes": serde_json::to_vec(&merged).unwrap().len() } },
            ],
        })),
    )
    .await;
    assert_eq!(status, 200);
    let rebase_result = rebase.unwrap().data.results.remove(0);
    assert!(rebase_result.ok, "rebase push: {:?}", rebase_result.error);
    assert_eq!(
        rebase_result.data.as_ref().unwrap().replaced_revision.as_deref(),
        Some(base_revision.as_str())
    );

    // 双端最终一致：bootstrap 头 + blob 内容即并集
    let (status, bootstrap) = request_json::<DataEnvelope<V2Bootstrap>>(
        &router,
        "GET",
        "/v2/sync/bootstrap?limit=10",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let snapshot = bootstrap.unwrap().data;
    let head = snapshot
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .unwrap();
    assert_eq!(head.content_hash.as_deref(), Some(merged_hash.as_str()));
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/v2/blobs/{merged_hash}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let final_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let final_payload: flowix_sync::V2ThreadPayload =
        serde_json::from_slice(&final_bytes).unwrap();
    assert_eq!(final_payload.messages.len(), 3, "三消息并集落库");
    // title：mobile 最后消息（200）早于 desktop（300）→ 远端（桌面）新 → 取远端标题？
    // merge 规则：remote_last(300) > local_last(200) → 取 remote.title=桌面会话
    assert_eq!(final_payload.title, "桌面会话");

    // history 完整：desktop put → merged put 共 2 版本
    let (status, history) = request_json::<DataEnvelope<V2History>>(
        &router,
        "GET",
        &format!("/v2/sync/history?noteId={thread_id}&limit=10"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(history.unwrap().data.revisions.len(), 2);
}
