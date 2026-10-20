//! FD-11 M5 本机模拟验收：双账号 × 双命名空间 agent × 内嵌中继的
//! 跨用户全链路——目录信任锚签名往返（verified）、byte-equal、
//! hops 递减、匿名配额执法、v2 usage 端点。
//! 验收标准见 docs/F5-CROSS-USER-NETWORK.md §8 M5。
//!
//! 对真中继复跑：`FLOWIX_TEST_RELAY_URL=https://relay.example.com
//! cargo test -p flowix-desktop --test cross_user_m5`——挑战登录自动
//! 幂等续跑；注意投递走匿名档（3/h/IP），同 IP 一小时内多次复跑需
//! 清库或等待计量窗口。

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use flowix_relay::envelope::canonical_bytes;
use flowix_relay::envelope::WireSignature;
use flowix_relay::routes::AppState;
use flowix_relay::store::Store;

use flowix_desktop::redaction::inbound::{
    inbound_dir, list_inbound, poll_agent_inbox, set_inbound_status, InboundStatus,
};
use flowix_desktop::redaction::outbox::{
    draft_from_preview, settle, take_confirmed_payload, write_record, AgentParty,
    OutboxStatus, RedactionOutboxRecord,
};
use flowix_desktop::redaction::transport::deliver_confirmed_envelope;
use flowix_desktop::redaction::RedactionPreviewReport;

/// 默认内嵌中继；设 FLOWIX_TEST_RELAY_URL（如 https://relay.example.com）
/// 则对真中继复跑本验收（返回 (url, 是否外部中继)）。
/// 外部模式下：同 username 重跑自动走挑战登录；重复注册 agent 幂等移交 token；
/// 配额断言跳过（计量窗口不可控）。
async fn spawn_relay() -> (String, bool) {
    if let Ok(url) = std::env::var("FLOWIX_TEST_RELAY_URL") {
        return (url.trim_end_matches('/').to_string(), true);
    }
    let router = flowix_relay::build_router(Arc::new(AppState::new(Store::in_memory())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), false)
}

fn report(text: &str) -> RedactionPreviewReport {
    RedactionPreviewReport {
        receipt_id: String::new(),
        final_text: text.to_string(),
        placeholder_map: Default::default(),
        l1_hits: vec![],
        attempts: 1,
        used_fallback: false,
        included_paths: vec!["note.md".into()],
        dropped_paths: vec![],
        truncated: false,
        target_level: "r2".into(),
        rule_set_version: "m5".into(),
    }
}

fn party(agent_id: &str, display: &str) -> AgentParty {
    AgentParty { agent_id: agent_id.into(), display_name: display.into() }
}

/// v2 账号：注册（新 username → 201 即得 token）；同 username 同公钥 →
/// 挑战登录（对真中继重复可跑）。
async fn register_account(client: &reqwest::Client, relay: &str, username: &str, key: &SigningKey) -> String {
    let response = client
        .post(format!("{relay}/v2/accounts"))
        .json(&serde_json::json!({
            "username": username,
            "devicePublicKey": BASE64.encode(key.verifying_key().as_bytes()),
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();
    if status == reqwest::StatusCode::CREATED {
        return body["deviceToken"].as_str().unwrap().to_string();
    }
    assert_eq!(status, reqwest::StatusCode::OK, "注册/登录 {username} 应 201/200: {body}");
    let challenge = body["challenge"].as_str().expect("登录挑战应携带 challenge").to_string();
    let user_id = body["userId"].as_str().expect("登录挑战应携带 userId").to_string();
    let signature = BASE64.encode(key.sign(challenge.as_bytes()).to_bytes());
    let response = client
        .post(format!("{relay}/v2/accounts/challenge"))
        .json(&serde_json::json!({
            "userId": user_id,
            "challenge": challenge,
            "signature": signature,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "挑战登录 {username} 应成功");
    response.json::<serde_json::Value>().await.unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 账号下注册命名空间收件通道（服务端归一 slug）→ 收件 deviceToken。
async fn register_agent(client: &reqwest::Client, relay: &str, account_token: &str, slug: &str) -> String {
    let response = client
        .post(format!("{relay}/v1/devices"))
        .bearer_auth(account_token)
        .json(&serde_json::json!({ "agentIds": [slug] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<serde_json::Value>().await.unwrap()["deviceToken"]
        .as_str()
        .unwrap()
        .to_string()
}

/// 发 public 目录卡（信任锚 = 真实公钥）。
async fn publish_card(client: &reqwest::Client, relay: &str, account_token: &str, username: &str, slug: &str, public_key: &str) {
    let response = client
        .put(format!("{relay}/v2/directory/{username}/{slug}"))
        .bearer_auth(account_token)
        .json(&serde_json::json!({
            "displayName": format!("{username}-{slug}"),
            "specialtyTags": ["m5"],
            "minRedactionLevel": "R1",
            "publicKey": public_key,
            "visibility": "public",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "发卡应成功");
}

/// 出站签名：对无签信封的规范化字节签名，写入留档（byte-equal：出口字节
/// = record.envelope() 重序列化，签名随档固化）。
fn sign_record(key: &SigningKey, record: &mut RedactionOutboxRecord) {
    let envelope = record.envelope();
    let signature = key.sign(&canonical_bytes(&envelope));
    record.signature = Some(WireSignature {
        alg: "ed25519".into(),
        key_id: BASE64.encode(key.verifying_key().as_bytes()),
        value: BASE64.encode(signature.to_bytes()),
    });
}

async fn confirm_and_deliver(
    root: &std::path::Path,
    record: &RedactionOutboxRecord,
    relay: &str,
) -> flowix_desktop::redaction::transport::RelayDeliverResult {
    write_record(root, record).unwrap();
    settle(root, &record.receipt_id, OutboxStatus::Confirmed).unwrap();
    deliver_confirmed_envelope(root, &record.receipt_id, relay).await.unwrap()
}

#[tokio::test]
async fn cross_user_m5_signed_roundtrip_with_directory_trust_anchor() {
    let (relay, external_relay) = spawn_relay().await;
    let client = reqwest::Client::new();
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();

    // ---- 两账号 × 命名空间 agent × public 目录卡（互为信任锚） ----
    let key_a = SigningKey::from_bytes(&[1u8; 32]);
    let key_b = SigningKey::from_bytes(&[2u8; 32]);
    let public_key_a = BASE64.encode(key_a.verifying_key().as_bytes());
    let public_key_b = BASE64.encode(key_b.verifying_key().as_bytes());
    let account_a = register_account(&client, &relay, "alice", &key_a).await;
    let account_b = register_account(&client, &relay, "bob", &key_b).await;
    let inbox_a = register_agent(&client, &relay, &account_a, "researcher").await;
    let inbox_b = register_agent(&client, &relay, &account_b, "observer").await;
    publish_card(&client, &relay, &account_a, "alice", "researcher", &public_key_a).await;
    publish_card(&client, &relay, &account_b, "bob", "observer", &public_key_b).await;

    // ---- 1. Alice 签名咨询 → Bob（中继经 Bob 侧信任锚 = alice 卡公钥验签） ----
    let mut consultation = draft_from_preview(
        "Q3 复购下滑原因?",
        &["entry_q3".into()],
        &report("跨用户咨询 ⟦PHONE-1⟧ 复购下滑"),
        chrono::Utc::now().timestamp_millis(),
    )
    .with_parties(party("alice/researcher", "Alice 研究员"), party("bob/observer", "Bob 观察员"));
    sign_record(&key_a, &mut consultation);
    let consultation_receipt = consultation.receipt_id.clone();

    // byte-equal 纪律（含签名块）：唯一出口字节 == 留档信封重序列化。
    let expected_bytes = consultation.envelope_bytes();
    write_record(root_a.path(), &consultation).unwrap();
    settle(root_a.path(), &consultation_receipt, OutboxStatus::Confirmed).unwrap();
    let outbound_bytes = take_confirmed_payload(root_a.path(), &consultation_receipt).unwrap();
    assert_eq!(outbound_bytes, expected_bytes, "出口字节与留档逐字节一致");
    let delivered = deliver_confirmed_envelope(root_a.path(), &consultation_receipt, &relay)
        .await
        .unwrap();
    assert_eq!(delivered.http_status, 200, "信任锚验签通过应投递成功");
    assert_eq!(delivered.byte_length, expected_bytes.len());

    // ---- 2. Bob 收件：验签结论 verified，跳数 2 ----
    let pulled = poll_agent_inbox(root_b.path(), &relay, &inbox_b, "bob/observer")
        .await
        .unwrap();
    assert_eq!(pulled.stored, 1);
    assert_eq!(pulled.rejected, 0);
    let inbound_b = list_inbound(&inbound_dir(root_b.path()));
    assert_eq!(inbound_b.len(), 1);
    let consult_inbound = &inbound_b[0];
    assert_eq!(consult_inbound.signature_status.as_deref(), Some("verified"), "经 alice 卡公钥验签应通过");
    assert_eq!(consult_inbound.from_agent_id, "alice/researcher");
    assert_eq!(consult_inbound.disclosure.hops_remaining, 2);
    set_inbound_status(&inbound_dir(root_b.path()), &consultation_receipt, InboundStatus::Accepted).unwrap();

    // ---- 3. Bob 签名回程：hops 2-1=1，inReplyTo 指向原咨询 ----
    let mut reply = draft_from_preview(
        "回答：Q3 复购下滑",
        &["entry_q3".into()],
        &report("主因 ⟦PHONE-1⟧ 客服响应慢"),
        chrono::Utc::now().timestamp_millis(),
    )
    .with_parties(party("bob/observer", "Bob 观察员"), party("alice/researcher", "Alice 研究员"))
    .with_reply(consultation_receipt.clone(), consult_inbound.disclosure.hops_remaining - 1);
    sign_record(&key_b, &mut reply);
    let reply_receipt = reply.receipt_id.clone();
    let reply_envelope = reply.envelope();
    assert_eq!(reply_envelope.payload.kind, "reply");
    assert_eq!(reply_envelope.disclosure.hops_remaining, 1, "回程跳数递减");
    let reply_delivered = confirm_and_deliver(root_b.path(), &reply, &relay).await;
    assert_eq!(reply_delivered.http_status, 200);

    // ---- 4. Alice 收到回答：verified + hops 1 + inReplyTo ----
    poll_agent_inbox(root_a.path(), &relay, &inbox_a, "alice/researcher")
        .await
        .unwrap();
    let inbound_a = list_inbound(&inbound_dir(root_a.path()));
    assert_eq!(inbound_a.len(), 1);
    let reply_inbound = &inbound_a[0];
    assert_eq!(reply_inbound.payload_kind, "reply");
    assert_eq!(reply_inbound.in_reply_to.as_deref(), Some(consultation_receipt.as_str()));
    assert_eq!(reply_inbound.signature_status.as_deref(), Some("verified"), "经 bob 卡公钥验签应通过");
    assert_eq!(reply_inbound.disclosure.hops_remaining, 1);

    // ---- 5. 匿名配额执法：本测试已匿名投递 2 封（咨询+回程，同 IP 桶 3/h），
    //         第 3 封仍可投，第 4 封被 429 拒且留档保持 confirmed 可重试。
    //         外部中继的计量窗口不可控，跳过本段。 ----
    if !external_relay {
        let mut filler = draft_from_preview(
            "配额探测",
            &["e".into()],
            &report("配额探测 ⟦PHONE-1⟧"),
            chrono::Utc::now().timestamp_millis(),
        )
        .with_parties(party("alice/researcher", ""), party("bob/observer", ""));
        write_record(root_a.path(), &filler).unwrap();
        let filler_receipt = filler.receipt_id.clone();
        settle(root_a.path(), &filler_receipt, OutboxStatus::Confirmed).unwrap();
        let third = deliver_confirmed_envelope(root_a.path(), &filler_receipt, &relay).await.unwrap();
        assert_eq!(third.http_status, 200, "第 3 封匿名投递仍在配额内");

        let mut fourth = draft_from_preview(
            "配额触顶",
            &["e".into()],
            &report("配额触顶 ⟦PHONE-1⟧"),
            chrono::Utc::now().timestamp_millis(),
        )
        .with_parties(party("alice/researcher", ""), party("bob/observer", ""));
        let fourth_receipt = fourth.receipt_id.clone();
        write_record(root_a.path(), &fourth).unwrap();
        settle(root_a.path(), &fourth_receipt, OutboxStatus::Confirmed).unwrap();
        let rejected = deliver_confirmed_envelope(root_a.path(), &fourth_receipt, &relay)
            .await
            .unwrap_err();
        assert!(rejected.contains("RELAY_REJECTED:429"), "第 4 封应被配额拒绝: {rejected}");
        // 429 后留档保持 confirmed（幂等可重试）。
        let after = flowix_desktop::redaction::read_record(root_a.path(), &fourth_receipt).unwrap();
        assert_eq!(after.status, OutboxStatus::Confirmed);
    }

    // ---- 6. v2 usage 端点可用（账号视角：本测试投递走匿名档，计量在 IP 桶） ----
    let response = client
        .get(format!("{relay}/v2/usage"))
        .bearer_auth(&account_a)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let usage: serde_json::Value = response.json().await.unwrap();
    assert_eq!(usage["windowSecs"].as_i64(), Some(3600));
    assert_eq!(usage["deliver"]["limit"].as_i64(), Some(30));
}
