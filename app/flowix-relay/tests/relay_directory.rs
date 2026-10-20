//! FD-11 v2 目录集成测试：公开卡发布/搜索/详情、三档可见性、投递执法、限流。
//! 验收标准（docs/F5-CROSS-USER-NETWORK.md §8 M3：三档可见性行为断言）。

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::SigningKey;
use flowix_relay::envelope::{
    AgentRef, WireDisclosure, WireEnvelope, WirePayload, WireProvenance, ENVELOPE_VERSION,
};
use flowix_relay::routes::{AppState, QuotaConfig};
use flowix_relay::store::Store;

fn envelope_to(to: &str) -> WireEnvelope {
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
        payload: WirePayload { kind: "consultation".into(), text: "咨询…".into() },
        provenance: WireProvenance {
            pipeline_version: "1".into(),
            rule_set_version: "rs1".into(),
            used_fallback: false,
            confirmed_by: "user".into(),
            confirmed_at: 1,
            receipt_id: "rct_dir".into(),
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

/// 注册账号 + 注册命名空间 agent，返回 (accountToken, publicKeyBase64)。
async fn setup_alice_with_agent(client: &reqwest::Client, base: &str, slug: &str) -> (String, String) {
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let public_key = BASE64.encode(key.verifying_key().as_bytes());
    let response = client
        .post(format!("{base}/v2/accounts"))
        .json(&serde_json::json!({ "username": "alice", "devicePublicKey": public_key }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let token = response.json::<serde_json::Value>().await.unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();
    let response = client
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "agentIds": [slug] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    (token, public_key)
}

async fn publish_card(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    slug: &str,
    visibility: &str,
    display_name: &str,
    tags: &[&str],
) -> reqwest::StatusCode {
    let response = client
        .put(format!("{base}/v2/directory/alice/{slug}"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "displayName": display_name,
            "specialtyTags": tags,
            "minRedactionLevel": "R1",
            "publicKey": BASE64.encode([7u8; 32]),
            "visibility": visibility,
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    // 消费 body 避免连接悬挂。
    let _ = response.text().await;
    status
}

#[tokio::test]
async fn publish_search_detail_roundtrip() {
    let base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let client = reqwest::Client::new();
    let (token, _) = setup_alice_with_agent(&client, &base, "researcher").await;

    let status = publish_card(&client, &base, &token, "researcher", "public", "财报验证助手", &["finance", "research"]).await;
    assert_eq!(status, reqwest::StatusCode::OK, "发布 public 卡应成功");

    // 搜索：q 命中 displayName。
    let response = client
        .get(format!("{base}/v2/directory?q=财报"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    let cards = body["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["address"].as_str().unwrap(), "alice/researcher");
    assert_eq!(cards[0]["card"]["minRedactionLevel"].as_str().unwrap(), "R1");
    assert_eq!(cards[0]["visibility"].as_str().unwrap(), "public");

    // tag 过滤命中 / 未命中。
    let response = client.get(format!("{base}/v2/directory?tag=finance")).send().await.unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["cards"].as_array().unwrap().len(), 1);
    let response = client.get(format!("{base}/v2/directory?tag=nobody")).send().await.unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["cards"].as_array().unwrap().len(), 0);

    // 详情。
    let response = client.get(format!("{base}/v2/directory/alice/researcher")).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["displayName"].as_str().unwrap(), "财报验证助手");
}

#[tokio::test]
async fn visibility_tiers_search_detail_and_delivery() {
    let base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let client = reqwest::Client::new();
    let (token, _) = setup_alice_with_agent(&client, &base, "researcher").await;
    // 同账号再注册两个 agent。
    for slug in ["observer", "secretive"] {
        let response = client
            .post(format!("{base}/v1/devices"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "agentIds": [slug] }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
    let agent_token = client
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "agentIds": ["researcher", "observer", "secretive"] }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        publish_card(&client, &base, &token, "researcher", "public", "公开专家", &["pub"]).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(
        publish_card(&client, &base, &token, "observer", "unlisted", "暗名单专家", &["dark"]).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(
        publish_card(&client, &base, &token, "secretive", "private", "私密专家", &["priv"]).await,
        reqwest::StatusCode::OK
    );

    // 搜索只见 public（空 q 全量检索）。
    let body: serde_json::Value =
        client.get(format!("{base}/v2/directory")).send().await.unwrap().json().await.unwrap();
    let addresses: Vec<&str> = body["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|card| card["address"].as_str().unwrap())
        .collect();
    assert_eq!(addresses, vec!["alice/researcher"], "搜索只应返回 public 卡");

    // 详情：public/unlisted 200；private 404。
    assert_eq!(
        client.get(format!("{base}/v2/directory/alice/researcher")).send().await.unwrap().status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        client.get(format!("{base}/v2/directory/alice/observer")).send().await.unwrap().status(),
        reqwest::StatusCode::OK,
        "unlisted 持完整地址详情可达"
    );
    let response = client.get(format!("{base}/v2/directory/alice/secretive")).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "CARD_NOT_FOUND"
    );

    // 投递：public/unlisted 可达；private → RECIPIENT_UNKNOWN。
    for agent in ["alice/researcher", "alice/observer"] {
        let status = client
            .post(format!("{base}/v1/outbox"))
            .json(&envelope_to(agent))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, reqwest::StatusCode::OK, "{agent} 投递应可达");
    }
    let response = client
        .post(format!("{base}/v1/outbox"))
        .json(&envelope_to("alice/secretive"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "RECIPIENT_UNKNOWN",
        "private 卡的 agent 投递应按不存在拒收"
    );

    // 拉取验证三封中前两封入队（secretive 被拒）。
    let body: serde_json::Value = client
        .get(format!("{base}/v1/inbox?agentId=alice%2Fresearcher"))
        .bearer_auth(&agent_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["envelopes"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn publish_guards_namespace_and_fields() {
    let base = spawn_relay(Arc::new(AppState::new(Store::in_memory()))).await;
    let client = reqwest::Client::new();
    let (token, _) = setup_alice_with_agent(&client, &base, "researcher").await;

    // 异常 visibility → 422。
    let response = client
        .put(format!("{base}/v2/directory/alice/researcher"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "displayName": "x", "specialtyTags": [], "minRedactionLevel": "R1",
            "publicKey": BASE64.encode([7u8; 32]), "visibility": "hidden",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "VISIBILITY_INVALID"
    );

    // 未注册收件通道的 agent → 422。
    let response = client
        .put(format!("{base}/v2/directory/alice/ghost"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "displayName": "幽灵", "specialtyTags": [], "minRedactionLevel": "R1",
            "publicKey": BASE64.encode([7u8; 32]), "visibility": "public",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "AGENT_NOT_REGISTERED"
    );

    // bob 的 token 不能给 alice 发卡 → 403。
    let bob_key = SigningKey::from_bytes(&[9u8; 32]);
    let bob_response = client
        .post(format!("{base}/v2/accounts"))
        .json(&serde_json::json!({
            "username": "bob",
            "devicePublicKey": BASE64.encode(bob_key.verifying_key().as_bytes()),
        }))
        .send()
        .await
        .unwrap();
    let bob_token = bob_response.json::<serde_json::Value>().await.unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string();
    let response = client
        .put(format!("{base}/v2/directory/alice/researcher"))
        .bearer_auth(&bob_token)
        .json(&serde_json::json!({
            "displayName": "仿冒", "specialtyTags": [], "minRedactionLevel": "R1",
            "publicKey": BASE64.encode([9u8; 32]), "visibility": "public",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "NAMESPACE_MISMATCH"
    );

    // 无 token → 401。
    let response = client
        .put(format!("{base}/v2/directory/alice/researcher"))
        .json(&serde_json::json!({
            "displayName": "x", "specialtyTags": [], "minRedactionLevel": "R1",
            "publicKey": BASE64.encode([7u8; 32]), "visibility": "public",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn directory_write_quota_enforced() {
    let state = Arc::new(AppState {
        store: Store::in_memory(),
        quotas: QuotaConfig { directory_write: 1, ..QuotaConfig::default() },
    });
    let base = spawn_relay(state).await;
    let client = reqwest::Client::new();
    let (token, _) = setup_alice_with_agent(&client, &base, "researcher").await;

    assert_eq!(
        publish_card(&client, &base, &token, "researcher", "public", "第一版", &[]).await,
        reqwest::StatusCode::OK
    );
    // 第二次写（更新同一张卡）→ 429。
    let response = client
        .put(format!("{base}/v2/directory/alice/researcher"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "displayName": "第二版", "specialtyTags": [], "minRedactionLevel": "R1",
            "publicKey": BASE64.encode([7u8; 32]), "visibility": "public",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
}

/// FD-11 M4 验收：签名信封往返——目录卡信任锚验签通过；篡改拒收；
/// 未发卡（无信任锚）签名信封放行；无签名信封照旧（匿名档）。
#[tokio::test]
async fn signed_envelope_delivery_with_directory_trust_anchor() {
    use ed25519_dalek::Signer as _;
    use flowix_relay::envelope::{canonical_bytes, WireSignature};

    // 本测试匿名投递 4 次，放宽匿名配额避免误触 429。
    let state = Arc::new(AppState {
        store: Store::in_memory(),
        quotas: QuotaConfig { deliver_anonymous: 30, ..QuotaConfig::default() },
    });
    let base = spawn_relay(state).await;
    let client = reqwest::Client::new();
    let (token, public_key) = setup_alice_with_agent(&client, &base, "researcher").await;
    // 信任锚 = 账号真实公钥（由 seed [7;32] 派生，非 seed 本身）。
    let response = client
        .put(format!("{base}/v2/directory/alice/researcher"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "displayName": "签名专家", "specialtyTags": ["signed"],
            "minRedactionLevel": "R1", "publicKey": public_key, "visibility": "public",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let mut envelope = envelope_to("alice/researcher");
    envelope.from.agent_id = "alice/researcher".into();
    let signature = signing_key.sign(&canonical_bytes(&envelope));
    envelope.signature = Some(WireSignature {
        alg: "ed25519".into(),
        key_id: public_key.clone(),
        value: BASE64.encode(signature.to_bytes()),
    });
    // 正确签名 → 投递成功。
    let response = client.post(format!("{base}/v1/outbox")).json(&envelope).send().await.unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "信任锚验签通过应投递成功: {body}");

    // 篡改 payload 后签名失配 → SIGNATURE_INVALID。
    let mut tampered = envelope.clone();
    tampered.payload.text = "篡改内容".into();
    let response = client.post(format!("{base}/v1/outbox")).json(&tampered).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["code"].as_str().unwrap(),
        "SIGNATURE_INVALID"
    );

    // 未发卡的 from（无信任锚）：带签名放行；无签名照旧（v1 行为）。
    let mut stranger = envelope_to("alice/researcher");
    stranger.from.agent_id = "eve/ghost".into();
    let signature = signing_key.sign(&canonical_bytes(&stranger));
    stranger.signature = Some(WireSignature {
        alg: "ed25519".into(),
        key_id: public_key,
        value: BASE64.encode(signature.to_bytes()),
    });
    let response = client.post(format!("{base}/v1/outbox")).json(&stranger).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK, "无信任锚的签名信封应放行");
    let unsigned = envelope_to("alice/researcher");
    let response = client.post(format!("{base}/v1/outbox")).json(&unsigned).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK, "无签名信封（v1）照旧投递");
}
