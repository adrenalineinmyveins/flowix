//! M4b 网络往返集成测试（FD-10 里程碑，契约 docs/F5-NETWORK-RELAY.md §5/§6）。
//!
//! 全链路走真实通道：内嵌 flowix-relay axum server（127.0.0.1 随机端口）
//! + 真实文件留档（tempfile 笔记本 root），与真机差异仅在驱动层
//! （UI 确认门逻辑由 redaction-send-gate / agent-profile-inbox 组件测试覆盖）。
//!
//! 验证：A 发起咨询 → 确认 → 中继投递 → B 拉取留档 → 接受 →
//! 回答（回程信封）→ 中继投递 → A 拉取收到 kind=reply（inReplyTo + 跳数递减）。

use std::sync::Arc;

use flowix_desktop::redaction::inbound::{
    inbound_dir, list_inbound, poll_agent_inbox, register_device, set_inbound_status,
    InboundStatus,
};
use flowix_desktop::redaction::outbox::{
    draft_from_preview, settle, write_record, AgentParty, OutboxStatus,
};
use flowix_desktop::redaction::transport::deliver_confirmed_envelope;
use flowix_desktop::redaction::RedactionPreviewReport;
use flowix_relay::routes::AppState;
use flowix_relay::store::Store;

fn report(text: &str) -> RedactionPreviewReport {
    let mut map = std::collections::BTreeMap::new();
    map.insert("⟦PHONE-1⟧".to_string(), "13800138000".to_string());
    RedactionPreviewReport {
        receipt_id: String::new(),
        final_text: text.to_string(),
        placeholder_map: map,
        l1_hits: vec![],
        attempts: 1,
        used_fallback: false,
        included_paths: vec!["notes/q3.md".into()],
        dropped_paths: vec![],
        truncated: false,
        target_level: "r2".into(),
        rule_set_version: "l1-v1".into(),
        distill_mode: "full".into(),
    }
}

fn party(agent_id: &str, display: &str) -> AgentParty {
    AgentParty { agent_id: agent_id.into(), display_name: display.into() }
}

async fn spawn_relay() -> String {
    let router = flowix_relay::build_router(Arc::new(AppState::new(Store::in_memory())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    format!("http://{addr}")
}

/// 确认并投递一条出站留档（draft → confirmed → POST /v1/outbox）。
async fn confirm_and_deliver(
    root: &std::path::Path,
    record: &flowix_desktop::redaction::outbox::RedactionOutboxRecord,
    relay: &str,
) -> flowix_desktop::redaction::transport::RelayDeliverResult {
    write_record(root, record).unwrap();
    settle(root, &record.receipt_id, OutboxStatus::Confirmed).unwrap();
    deliver_confirmed_envelope(root, &record.receipt_id, relay).await.unwrap()
}

#[tokio::test]
async fn full_roundtrip_consultation_and_reply() {
    let relay = spawn_relay().await;
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();

    // 两台设备各自注册收件通道（不同 token，单设备语义）。
    let token_a = register_device(&relay, &["agt_a".into()], None).await.unwrap();
    let token_b = register_device(&relay, &["agt_b".into()], None).await.unwrap();
    assert_ne!(token_a, token_b);

    // ---- 1. A 发起咨询（sender=agt_a，收件方=agt_b）----
    let now_ms = chrono::Utc::now().timestamp_millis();
    let consultation = draft_from_preview(
        "Q3 复购下滑原因?",
        &["entry_q3".into()],
        &report("Q3 复购 ⟦PHONE-1⟧ 咨询下滑原因"),
        now_ms,
    )
    .with_parties(party("agt_a", "A 设备"), party("agt_b", "B 专家"));
    let consultation_receipt = consultation.receipt_id.clone();
    let consultation_envelope = consultation.envelope();
    assert_eq!(consultation_envelope.payload.kind, "consultation");
    assert_eq!(consultation_envelope.disclosure.hops_remaining, 2);
    let expected_bytes = consultation.envelope_bytes();

    let delivered = confirm_and_deliver(root_a.path(), &consultation, &relay).await;
    assert_eq!(delivered.http_status, 200);
    // byte-equal 纪律：投递字节长度 = 留档信封序列化。
    assert_eq!(delivered.byte_length, expected_bytes.len());

    // ---- 2. B 拉取收件箱：留档 pending，relay 队列经 ack 清空 ----
    let pulled = poll_agent_inbox(root_b.path(), &relay, &token_b, "agt_b")
        .await
        .unwrap();
    assert_eq!(pulled.stored, 1);
    assert_eq!(pulled.rejected, 0);
    let idle = poll_agent_inbox(root_b.path(), &relay, &token_b, "agt_b")
        .await
        .unwrap();
    assert_eq!(idle.stored + idle.duplicates + idle.rejected, 0);

    let inbound_b = list_inbound(&inbound_dir(root_b.path()));
    assert_eq!(inbound_b.len(), 1);
    let got = &inbound_b[0];
    assert_eq!(got.status, InboundStatus::Pending);
    assert_eq!(got.receipt_id, consultation_receipt);
    assert_eq!(got.from_agent_id, "agt_a");
    assert_eq!(got.to_agent_id, "agt_b");
    assert_eq!(got.payload_kind, "consultation");
    assert_eq!(got.in_reply_to, None);
    assert_eq!(got.payload_text, "Q3 复购 ⟦PHONE-1⟧ 咨询下滑原因");
    assert_eq!(got.disclosure.hops_remaining, 2);

    // ---- 3. B 接受咨询 ----
    set_inbound_status(&inbound_dir(root_b.path()), &consultation_receipt, InboundStatus::Accepted)
        .unwrap();
    assert_eq!(
        list_inbound(&inbound_dir(root_b.path()))[0].status,
        InboundStatus::Accepted
    );

    // ---- 4. B 写回答：回程信封（kind=reply，inReplyTo 指向原咨询，跳数 2-1=1）----
    let hops_inherited = got.disclosure.hops_remaining - 1;
    let reply = draft_from_preview(
        "回答：Q3 复购下滑",
        &["entry_q3".into()],
        &report("复购下滑主因是 ⟦PHONE-1⟧ 客服响应慢"),
        chrono::Utc::now().timestamp_millis(),
    )
    .with_parties(party("agt_b", "B 专家"), party("agt_a", "A 设备"))
    .with_reply(consultation_receipt.clone(), hops_inherited);
    let reply_envelope = reply.envelope();
    assert_eq!(reply_envelope.payload.kind, "reply");
    assert_eq!(reply_envelope.in_reply_to.as_deref(), Some(consultation_receipt.as_str()));
    assert_eq!(reply_envelope.disclosure.hops_remaining, 1);

    let reply_delivered = confirm_and_deliver(root_b.path(), &reply, &relay).await;
    assert_eq!(reply_delivered.http_status, 200);

    // ---- 5. A 拉取收件箱：收到回程信封 ----
    let pulled_a =
        poll_agent_inbox(root_a.path(), &relay, &token_a, "agt_a").await.unwrap();
    assert_eq!(pulled_a.stored, 1);

    let inbound_a = list_inbound(&inbound_dir(root_a.path()));
    assert_eq!(inbound_a.len(), 1);
    let reply_inbound = &inbound_a[0];
    assert_eq!(reply_inbound.payload_kind, "reply");
    assert_eq!(
        reply_inbound.in_reply_to.as_deref(),
        Some(consultation_receipt.as_str())
    );
    assert_eq!(reply_inbound.from_agent_id, "agt_b");
    assert_eq!(reply_inbound.to_agent_id, "agt_a");
    assert_eq!(reply_inbound.disclosure.hops_remaining, 1);
    assert_eq!(reply_inbound.status, InboundStatus::Pending);
    assert_eq!(reply_inbound.payload_text, "复购下滑主因是 ⟦PHONE-1⟧ 客服响应慢");

    // ---- 6. 双侧留档终态：A 出站 1 confirmed + A 入站 1 pending；B 出站 1 + 入站 1 accepted ----
    assert_eq!(
        flowix_desktop::redaction::outbox::list_records(root_a.path()).len(),
        1
    );
    assert_eq!(
        flowix_desktop::redaction::outbox::list_records(root_b.path()).len(),
        1
    );
}

/// 中继对未注册收件方的信封回 404 RECIPIENT_UNKNOWN（§3.1）：出站留档保持
/// confirmed 可重试，不产生半投递状态。
#[tokio::test]
async fn deliver_to_unregistered_recipient_stays_retryable() {
    let relay = spawn_relay().await;
    let root = tempfile::tempdir().unwrap();
    register_device(&relay, &["agt_a".into()], None).await.unwrap();

    let record = draft_from_preview(
        "q",
        &["e1".into()],
        &report("占位文本 ⟦PHONE-1⟧"),
        chrono::Utc::now().timestamp_millis(),
    )
    .with_parties(party("agt_a", "A"), party("agt_ghost", "未注册"));
    write_record(root.path(), &record).unwrap();
    settle(root.path(), &record.receipt_id, OutboxStatus::Confirmed).unwrap();

    let error = deliver_confirmed_envelope(root.path(), &record.receipt_id, &relay)
        .await
        .unwrap_err();
    assert_eq!(error, "RELAY_REJECTED:404");

    // 留档仍 confirmed（take 出口可重试），未产生副作用。
    let after = flowix_desktop::redaction::read_record(root.path(), &record.receipt_id).unwrap();
    assert_eq!(after.status, OutboxStatus::Confirmed);
}
