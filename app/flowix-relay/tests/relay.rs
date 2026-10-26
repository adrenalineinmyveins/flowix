//! 端到端 HTTP 集成测试：注册 → 投递 → 拉取 → ack，以及 §3.1 拒绝码全覆盖。
//! M2 验收标准（docs/F5-NETWORK-RELAY.md §7）。

use std::sync::Arc;

use flowix_relay::envelope::{
    AgentRef, WireDisclosure, WireEnvelope, WirePayload, WireProvenance, ENVELOPE_VERSION,
};
use flowix_relay::routes::AppState;
use flowix_relay::store::Store;

fn sample_envelope() -> WireEnvelope {
    WireEnvelope {
        envelope: ENVELOPE_VERSION.to_string(),
        from: AgentRef { agent_id: "agt_a".into(), display_name: "A".into() },
        to: AgentRef { agent_id: "agt_b".into(), display_name: "B".into() },
        in_reply_to: None,
        disclosure: WireDisclosure {
            redaction_level: "R2".into(),
            max_hops: 2,
            hops_remaining: 2,
            expires_at: 4_102_444_800,
            allowed_scopes: vec!["tag:finance/财报".into()],
        },
        payload: WirePayload { kind: "consultation".into(), text: "问题…".into(), cipher_text: None },
        provenance: WireProvenance {
            pipeline_version: "1".into(),
            rule_set_version: "rs1".into(),
            used_fallback: false,
            confirmed_by: "user".into(),
            confirmed_at: 1,
            receipt_id: "rct_1".into(),
        },
        signature: None,
    }
}

async fn spawn_relay() -> String {
    let router = flowix_relay::build_router(Arc::new(AppState::new(Store::in_memory())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

async fn post_json<T: serde::ser::Serialize>(
    client: &reqwest::Client,
    url: &str,
    body: &T,
) -> (reqwest::StatusCode, serde_json::Value) {
    let response = client.post(url).json(body).send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(serde_json::Value::Null))
}

async fn post_json_authed<T: serde::ser::Serialize>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    body: &T,
) -> (reqwest::StatusCode, serde_json::Value) {
    let response = client
        .post(url)
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn full_roundtrip_register_outbox_inbox_ack() {
    let base = spawn_relay().await;
    let client = reqwest::Client::new();

    // 注册 B 设备。
    let (status, body) = post_json(
        &client,
        &format!("{base}/v1/devices"),
        &serde_json::json!({ "agentIds": ["agt_b"] }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let token = body["deviceToken"].as_str().unwrap().to_string();

    // A → B 投递。
    let (status, body) = post_json(&client, &format!("{base}/v1/outbox"), &sample_envelope()).await;
    assert_eq!(status, reqwest::StatusCode::OK, "body: {body}");
    let queue_id = body["queueId"].as_str().unwrap().to_string();

    // B 拉取（带 token）。
    let response = client
        .get(format!("{base}/v1/inbox?agentId=agt_b"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let inbox: serde_json::Value = response.json().await.unwrap();
    assert_eq!(inbox["envelopes"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["envelopes"][0]["queueId"].as_str().unwrap(), queue_id);
    assert_eq!(
        inbox["envelopes"][0]["envelope"]["payload"]["text"].as_str().unwrap(),
        "问题…"
    );

    // ack 出队后再拉为空。
    let (status, body) = post_json_authed(
        &client,
        &format!("{base}/v1/inbox/ack"),
        &token,
        &serde_json::json!({ "agentId": "agt_b", "queueIds": [queue_id] }),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(body["removed"].as_u64().unwrap(), 1);

    let response = client
        .get(format!("{base}/v1/inbox?agentId=agt_b"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    let inbox: serde_json::Value = response.json().await.unwrap();
    assert_eq!(inbox["envelopes"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn inbox_requires_owned_token() {
    let base = spawn_relay().await;
    let client = reqwest::Client::new();

    let (_, body) = post_json(
        &client,
        &format!("{base}/v1/devices"),
        &serde_json::json!({ "agentIds": ["agt_b"] }),
    )
    .await;
    let token = body["deviceToken"].as_str().unwrap().to_string();

    // 无 token → 401。
    let response = client.get(format!("{base}/v1/inbox?agentId=agt_b")).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 其他 agent → 403。
    let response = client
        .get(format!("{base}/v1/inbox?agentId=agt_other"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn recipient_unknown_rejected() {
    let base = spawn_relay().await;
    let client = reqwest::Client::new();

    let (status, body) = post_json(&client, &format!("{base}/v1/outbox"), &sample_envelope()).await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    assert_eq!(body["code"].as_str().unwrap(), "RECIPIENT_UNKNOWN");
}

#[tokio::test]
async fn reject_codes_over_http() {
    let base = spawn_relay().await;
    let client = reqwest::Client::new();
    post_json(
        &client,
        &format!("{base}/v1/devices"),
        &serde_json::json!({ "agentIds": ["agt_b"] }),
    )
    .await;

    let cases: Vec<(&'static str, WireEnvelope)> = {
        let mut cases = Vec::new();
        let mut e = sample_envelope();
        e.envelope = "flowix.consult.v0".into();
        cases.push(("ENVELOPE_VERSION_UNSUPPORTED", e));

        let mut e = sample_envelope();
        e.disclosure.redaction_level = "R0".into();
        cases.push(("DISCLOSURE_INCOMPLETE", e));

        let mut e = sample_envelope();
        e.payload.text = "x".repeat(64 * 1024 + 1);
        cases.push(("PAYLOAD_TOO_LARGE", e));

        let mut e = sample_envelope();
        e.disclosure.expires_at = 1;
        cases.push(("EXPIRED", e));

        cases
    };

    for (expected_code, envelope) in cases {
        let (status, body) = post_json(&client, &format!("{base}/v1/outbox"), &envelope).await;
        assert_eq!(status, reqwest::StatusCode::UNPROCESSABLE_ENTITY, "case {expected_code}");
        assert_eq!(body["code"].as_str().unwrap(), expected_code);
    }
}

#[tokio::test]
async fn health_endpoint() {
    let base = spawn_relay().await;
    let response = reqwest::get(format!("{base}/v1/health")).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ok");
    assert_eq!(body["envelopeVersion"].as_str().unwrap(), ENVELOPE_VERSION);
}
