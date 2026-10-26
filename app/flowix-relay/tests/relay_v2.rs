//! FD-11 v2 多租户集成测试：账号全流程、命名空间投递、配额执法、重启恢复。
//! 验收标准（docs/F5-CROSS-USER-NETWORK.md §8 M2）。

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use flowix_relay::envelope::{
    AgentRef, WireDisclosure, WireEnvelope, WirePayload, WireProvenance, ENVELOPE_VERSION,
};
use flowix_relay::routes::{AppState, QuotaConfig};
use flowix_relay::store::Store;

fn namespaced_envelope(to: &str) -> WireEnvelope {
    WireEnvelope {
        envelope: ENVELOPE_VERSION.to_string(),
        from: AgentRef { agent_id: "bob/observer".into(), display_name: "B".into() },
        to: AgentRef { agent_id: to.into(), display_name: "A".into() },
        in_reply_to: None,
        disclosure: WireDisclosure {
            redaction_level: "R2".into(),
            max_hops: 2,
            hops_remaining: 2,
            expires_at: 4_102_444_800,
            allowed_scopes: Vec::new(),
        },
        payload: WirePayload { kind: "consultation".into(), text: "跨用户咨询…".into(), cipher_text: None },
        provenance: WireProvenance {
            pipeline_version: "1".into(),
            rule_set_version: "rs1".into(),
            used_fallback: false,
            confirmed_by: "user".into(),
            confirmed_at: 1,
            receipt_id: "rct_v2".into(),
        },
        signature: None,
    }
}

async fn spawn_relay(state: Arc<AppState>) -> String {
    let router = flowix_relay::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

/// 注册账号并返回 (userId, deviceToken)；公钥来自 SigningKey。
async fn register_account(
    client: &reqwest::Client,
    base: &str,
    username: &str,
    key: &SigningKey,
) -> (String, String) {
    let response = client
        .post(format!("{base}/v2/accounts"))
        .json(&serde_json::json!({
            "username": username,
            "devicePublicKey": BASE64.encode(key.verifying_key().as_bytes()),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED, "账号注册应返回 201");
    let body: serde_json::Value = response.json().await.unwrap();
    (body["userId"].as_str().unwrap().to_string(), body["deviceToken"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn account_register_then_challenge_login() {
    let base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let client = reqwest::Client::new();
    let key = SigningKey::from_bytes(&[7u8; 32]);

    // 同公钥再次注册 → 登录挑战（无 token）。
    register_account(&client, &base, "alice", &key).await;
    let response = client
        .post(format!("{base}/v2/accounts"))
        .json(&serde_json::json!({
            "username": "alice",
            "devicePublicKey": BASE64.encode(key.verifying_key().as_bytes()),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    let user_id = body["userId"].as_str().unwrap().to_string();
    let challenge = body["challenge"].as_str().unwrap().to_string();
    assert!(body["deviceToken"].is_null());

    // 签名挑战换发 token。
    let signature = BASE64.encode(key.sign(challenge.as_bytes()).to_bytes());
    let response = client
        .post(format!("{base}/v2/accounts/challenge"))
        .json(&serde_json::json!({ "userId": user_id, "challenge": challenge, "signature": signature }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["deviceToken"].as_str().unwrap().starts_with("dev_"));

    // 异公钥抢注 → 409。
    let attacker = SigningKey::from_bytes(&[9u8; 32]);
    let response = client
        .post(format!("{base}/v2/accounts"))
        .json(&serde_json::json!({
            "username": "alice",
            "devicePublicKey": BASE64.encode(attacker.verifying_key().as_bytes()),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(), "USERNAME_TAKEN");
}

#[tokio::test]
async fn namespaced_roundtrip() {
    let base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let client = reqwest::Client::new();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (_, account_token) = register_account(&client, &base, "alice", &key).await;

    // Bearer 账号 token 注册 agent：裸 slug 自动归一到 alice/researcher。
    let response = client
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&account_token)
        .json(&serde_json::json!({ "agentIds": ["researcher"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let agent_token =
        response.json::<serde_json::Value>().await.unwrap()["deviceToken"].as_str().unwrap().to_string();

    // 跨账号（bob）投递到 alice/researcher。
    let response = client
        .post(format!("{base}/v1/outbox"))
        .json(&namespaced_envelope("alice/researcher"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let queue_id = response.json::<serde_json::Value>().await.unwrap()["queueId"].as_str().unwrap().to_string();

    // alice 侧拉取 + ack。
    let response = client
        .get(format!("{base}/v1/inbox?agentId=alice%2Fresearcher"))
        .bearer_auth(&agent_token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK, "命名空间拉取应通过");
    let inbox: serde_json::Value = response.json().await.unwrap();
    assert_eq!(inbox["envelopes"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["envelopes"][0]["queueId"].as_str().unwrap(), queue_id);

    let response = client
        .post(format!("{base}/v1/inbox/ack"))
        .bearer_auth(&agent_token)
        .json(&serde_json::json!({ "agentId": "alice/researcher", "queueIds": [queue_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    // 未注册的命名空间 agent → 404。
    let response = client
        .post(format!("{base}/v1/outbox"))
        .json(&namespaced_envelope("alice/ghost"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn namespace_mismatch_rejected() {
    let base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let client = reqwest::Client::new();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (_, bob_token) = register_account(&client, &base, "bob", &key).await;

    // bob 的 token 不能注册 alice/ 前缀的 agent。
    let response = client
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&bob_token)
        .json(&serde_json::json!({ "agentIds": ["alice/researcher"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "NAMESPACE_MISMATCH"
    );
}

#[tokio::test]
async fn anonymous_and_poll_quotas_enforced() {
    // 注入小配额：匿名投递 2/h，拉取 1/h。
    let state = Arc::new(AppState::with_quotas(
        Store::in_memory(),
        QuotaConfig {
            window_secs: 3600,
            deliver_authenticated: 30,
            deliver_anonymous: 2,
            poll: 1,
            ..QuotaConfig::default()
        },
    ));
    let base = spawn_relay(state).await;
    let client = reqwest::Client::new();

    // 匿名收件设备（v1 流程）。
    let response = client
        .post(format!("{base}/v1/devices"))
        .json(&serde_json::json!({ "agentIds": ["agt_b"] }))
        .send()
        .await
        .unwrap();
    let token =
        response.json::<serde_json::Value>().await.unwrap()["deviceToken"].as_str().unwrap().to_string();

    // 匿名投递 2 次成功（收件人 agt_b 已注册；匿名按 IP 计量）。
    for _ in 0..2 {
        let response = client
            .post(format!("{base}/v1/outbox"))
            .json(&namespaced_envelope("agt_b"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
    // 第 3 次 → 429。
    let response = client
        .post(format!("{base}/v1/outbox"))
        .json(&namespaced_envelope("agt_b"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "RATE_LIMITED"
    );

    // 拉取 1 次成功（含未 ack 的 2 封），第 2 次 → 429。
    let response = client
        .get(format!("{base}/v1/inbox?agentId=agt_b"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response = client
        .get(format!("{base}/v1/inbox?agentId=agt_b"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn usage_endpoint_reports_window() {
    let state = Arc::new(AppState::with_quotas(
        Store::in_memory(),
        QuotaConfig {
            window_secs: 3600,
            deliver_authenticated: 30,
            deliver_anonymous: 3,
            poll: 120,
            ..QuotaConfig::default()
        },
    ));
    let base = spawn_relay(state).await;
    let client = reqwest::Client::new();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let (_, token) = register_account(&client, &base, "alice", &key).await;

    // 注册 agent 并用账号 token 认证投递一次。
    client
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "agentIds": ["researcher"] }))
        .send()
        .await
        .unwrap();
    let response = client
        .post(format!("{base}/v1/outbox"))
        .bearer_auth(&token)
        .json(&namespaced_envelope("alice/researcher"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let response = client
        .get(format!("{base}/v2/usage"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let usage: serde_json::Value = response.json().await.unwrap();
    assert_eq!(usage["deliver"]["used"].as_i64().unwrap(), 1);
    assert_eq!(usage["deliver"]["limit"].as_i64().unwrap(), 30);
    assert_eq!(usage["poll"]["used"].as_i64().unwrap(), 0);

    // 无 token → 401。
    let response = client.get(format!("{base}/v2/usage")).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn restart_recovers_accounts_queue_and_usage() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("relay-v2.db");
    let key = SigningKey::from_bytes(&[7u8; 32]);

    let (user_id, base) = {
        let state = Arc::new(AppState::new(Store::persisted(&db_path)));
        let base = spawn_relay(state).await;
        let client = reqwest::Client::new();
        let (user_id, token) = register_account(&client, &base, "alice", &key).await;
        client
            .post(format!("{base}/v1/devices"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "agentIds": ["researcher"] }))
            .send()
            .await
            .unwrap();
        client
            .post(format!("{base}/v1/outbox"))
            .json(&namespaced_envelope("alice/researcher"))
            .send()
            .await
            .unwrap();
        (user_id, base)
    };
    drop(base);

    // 「重启」：同一 SQLite 文件新实例。
    let state = Arc::new(AppState::new(Store::persisted(&db_path)));
    let base = spawn_relay(state).await;
    let client = reqwest::Client::new();

    // 账号仍在：挑战登录可用。
    let response = client
        .post(format!("{base}/v2/accounts"))
        .json(&serde_json::json!({
            "username": "alice",
            "devicePublicKey": BASE64.encode(key.verifying_key().as_bytes()),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["userId"].as_str().unwrap(), user_id);
    let challenge = body["challenge"].as_str().unwrap().to_string();
    let signature = BASE64.encode(key.sign(challenge.as_bytes()).to_bytes());
    let response = client
        .post(format!("{base}/v2/accounts/challenge"))
        .json(&serde_json::json!({ "userId": user_id, "challenge": challenge, "signature": signature }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK, "重启后挑战登录应可用");
    let token = response.json::<serde_json::Value>().await.unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();

    // 队列仍在：重启后重夺 agent（单设备语义）→ 拉取到重启前投递的信封。
    let response = client
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "agentIds": ["researcher"] }))
        .send()
        .await
        .unwrap();
    let agent_token = response.json::<serde_json::Value>().await.unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();
    let response = client
        .get(format!("{base}/v1/inbox?agentId=alice%2Fresearcher"))
        .bearer_auth(&agent_token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let inbox: serde_json::Value = response.json().await.unwrap();
    assert_eq!(inbox["envelopes"].as_array().unwrap().len(), 1, "重启后队列应保留");

    // 计量仍在：usage 反映重启前的拉取。
    let response = client
        .get(format!("{base}/v2/usage"))
        .bearer_auth(&agent_token)
        .send()
        .await
        .unwrap();
    let usage: serde_json::Value = response.json().await.unwrap();
    assert_eq!(usage["poll"]["used"].as_i64().unwrap(), 1);
}

/// FD-11 P2-7 联邦一跳转发：A 中继无此 agent → 转发到 peer B 落队；
/// 已途径列表防环（X-Forwarded-From 含 B → 不再转投 → 404）。
#[tokio::test]
async fn federation_one_hop_forward_and_loop_guard() {
    let client = reqwest::Client::new();

    // B 中继：注册 carol/analyst。
    let b_base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let b_token = client
        .post(format!("{b_base}/v1/devices"))
        .json(&serde_json::json!({ "agentIds": ["carol/analyst"] }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();

    // A 中继：无该 agent，peers = [B]。
    let a_base = spawn_relay(Arc::new(AppState::with_peers(
        Store::in_memory(),
        vec![b_base.clone()],
    )))
    .await;

    // 向 A 投递 → A 转发 B，2xx + 伪 queueId `fed:*`。
    let response = client
        .post(format!("{a_base}/v1/outbox"))
        .json(&namespaced_envelope("carol/analyst"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK, "联邦转发应成功");
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["queueId"].as_str().unwrap().starts_with("fed:"));

    // B 侧收件可拉到。
    let response = client
        .get(format!("{b_base}/v1/inbox?agentId=carol%2Fanalyst"))
        .bearer_auth(&b_token)
        .send()
        .await
        .unwrap();
    let inbox: serde_json::Value = response.json().await.unwrap();
    assert_eq!(inbox["envelopes"].as_array().unwrap().len(), 1, "B 应收到转发信封");

    // 防环：请求已途径 B（X-Forwarded-From）→ A 不再向 B 转发 → 404。
    let response = client
        .post(format!("{a_base}/v1/outbox"))
        .header("X-Forwarded-From", &b_base)
        .json(&namespaced_envelope("nobody/none"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND, "已途径对端应防环拒转");

    // 无 peers 的中继：未知 agent 直接 404（不参与联邦）。
    let c_base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let response = client
        .post(format!("{c_base}/v1/outbox"))
        .json(&namespaced_envelope("carol/analyst"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}
