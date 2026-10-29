//! AGENT-MARKET M3（D-5）：订阅 = 本地关注列表。
//!
//! 订阅时拉单卡详情存快照（address/名称/标签/R 级/autoAnswer/公钥/中继），
//! 落 `~/.flowix/redaction/subscriptions.json`（全局，与 account.json 同层，
//! 跨笔记本可 consult）；动作仅订阅/退订/手动刷新，无服务端推送、无轮询。
//! 快照供 `consult_targets` 枚举（D-6），地址即完整 `user/slug@relay`。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::directory::{self, DirectoryCard};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribedCard {
    /// `user/slug`（不含 @relay，中继独立字段承载）。
    pub address: String,
    pub relay_url: String,
    pub display_name: String,
    #[serde(default)]
    pub specialty_tags: Vec<String>,
    #[serde(default)]
    pub min_redaction_level: String,
    #[serde(default)]
    pub auto_answer: String,
    #[serde(default)]
    pub card_public_key: String,
    pub subscribed_at: i64,
    pub refreshed_at: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// 单测注入：thread_local 路径隔离真实 HOME（Windows 的 home_dir 不读
// env，改 HOME 无效且会污染真实数据）。跨模块测试经 test_support 注入。
#[cfg(test)]
thread_local! {
    static TEST_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) mod test_support {
    /// 注入/清除测试路径（None = 回落真实 HOME）。
    pub fn set_test_path(path: Option<std::path::PathBuf>) {
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = path);
    }

    /// 直接落订阅条目（绕过触网的 subscribe，供跨模块测试）。
    pub fn set_entries(cards: Vec<super::SubscribedCard>) {
        super::write_all(&cards).expect("test entries write");
    }
}

pub fn subscriptions_path() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(path) = TEST_PATH.with(|cell| cell.borrow().clone()) {
        return Ok(path);
    }
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction").join("subscriptions.json"))
}

fn read_all() -> Result<Vec<SubscribedCard>, String> {
    let path = subscriptions_path()?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(Vec::new());
    };
    serde_json::from_slice(&bytes).map_err(|e| format!("SUBSCRIPTIONS_INVALID: {e}"))
}

fn write_all(cards: &[SubscribedCard]) -> Result<(), String> {
    let path = subscriptions_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("SUBSCRIPTIONS_DIR_FAILED: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(cards)
        .map_err(|e| format!("SUBSCRIPTIONS_SERIALIZE_FAILED: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("SUBSCRIPTIONS_WRITE_FAILED: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("SUBSCRIPTIONS_WRITE_FAILED: {e}"))
}

/// `user/slug` 拆解。
fn split_address(address: &str) -> Result<(String, String), String> {
    let trimmed = address.trim();
    let (user, slug) = trimmed
        .split_once('/')
        .ok_or_else(|| format!("SUBSCRIPTION_ADDRESS_INVALID: {trimmed}"))?;
    if user.is_empty() || slug.is_empty() || slug.contains('/') {
        return Err(format!("SUBSCRIPTION_ADDRESS_INVALID: {trimmed}"));
    }
    Ok((user.to_string(), slug.to_string()))
}

fn snapshot_from_card(card: &DirectoryCard, relay_url: &str, now: i64, previous: Option<&SubscribedCard>) -> SubscribedCard {
    SubscribedCard {
        address: card.address.clone(),
        relay_url: relay_url.to_string(),
        display_name: card.display_name.clone(),
        specialty_tags: card
            .card
            .get("specialtyTags")
            .and_then(|value| value.as_array())
            .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default(),
        min_redaction_level: card
            .card
            .get("minRedactionLevel")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        auto_answer: card
            .card
            .get("autoAnswer")
            .and_then(|value| value.as_str())
            .unwrap_or("off")
            .to_string(),
        card_public_key: card
            .card
            .get("publicKey")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        subscribed_at: previous.map(|item| item.subscribed_at).unwrap_or(now),
        refreshed_at: now,
    }
}

/// 订阅/刷新共用的远端拉取：GET 单卡详情 → 快照。远端 private/不存在 →
/// `SUBSCRIPTION_NOT_FOUND`；网络失败原样透传（快照不动）。
async fn fetch_snapshot(address: &str, relay_url: &str) -> Result<SubscribedCard, String> {
    let (user, slug) = split_address(address)?;
    let card = directory::card(Some(relay_url), &user, &slug).await?;
    let now = now_unix();
    let cards = read_all()?;
    let previous = cards.iter().find(|item| item.address == card.address);
    Ok(snapshot_from_card(&card, relay_url, now, previous))
}

/// 订阅：拉单卡详情存快照；同 address 重复订阅 = 覆盖更新（幂等）。
pub async fn subscribe(address: &str, relay_url: Option<&str>) -> Result<SubscribedCard, String> {
    let relay = match relay_url {
        Some(url) if !url.trim().is_empty() => url.trim().to_string(),
        _ => super::account::read_account()
            .map(|account| account.relay_url)
            .ok_or_else(|| "ACCOUNT_NOT_CONFIGURED".to_string())?,
    };
    let snapshot = fetch_snapshot(address, &relay).await?;
    let mut cards = read_all()?;
    match cards.iter().position(|item| item.address == snapshot.address) {
        Some(index) => cards[index] = snapshot.clone(),
        None => cards.push(snapshot.clone()),
    }
    write_all(&cards)?;
    Ok(snapshot)
}

/// 刷新已有订阅的快照；未订阅 → `NOT_SUBSCRIBED`。
pub async fn refresh(address: &str) -> Result<SubscribedCard, String> {
    let cards = read_all()?;
    let current = cards
        .iter()
        .find(|item| item.address == address.trim())
        .ok_or_else(|| format!("NOT_SUBSCRIBED: {address}"))?;
    let snapshot = fetch_snapshot(&current.address, &current.relay_url).await?;
    let mut cards = read_all()?;
    if let Some(index) = cards.iter().position(|item| item.address == snapshot.address) {
        cards[index] = snapshot.clone();
        write_all(&cards)?;
    }
    Ok(snapshot)
}

/// 退订：删除快照条目。
pub fn unsubscribe(address: &str) -> Result<(), String> {
    let mut cards = read_all()?;
    let before = cards.len();
    cards.retain(|item| item.address != address.trim());
    if cards.len() == before {
        return Err(format!("NOT_SUBSCRIBED: {address}"));
    }
    write_all(&cards)
}

pub fn list() -> Vec<SubscribedCard> {
    read_all().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入临时 subscriptions 路径（thread_local，并行安全，不碰真实 HOME）。
    fn with_temp_store(action: impl FnOnce(&std::path::Path)) {
        let dir = tempfile::tempdir().unwrap();
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = Some(dir.path().join("subscriptions.json")));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(dir.path())));
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = None);
        result.unwrap();
    }

    /// 单连接 mock relay：对任意请求回固定状态 + JSON 体（阻塞线程随测试
    /// 进程退出回收）。覆盖 subscribe/refresh 的远端交互层。
    fn spawn_mock_relay(status: &'static str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0u8; 8192];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        url
    }

    const CARD_JSON: &str = r#"{
        "address": "alice/caibao",
        "username": "alice",
        "agentSlug": "caibao",
        "displayName": "财报解读助手",
        "visibility": "public",
        "card": {
            "specialtyTags": ["finance/财报"],
            "minRedactionLevel": "R2",
            "autoAnswer": "off",
            "publicKey": "pk1"
        },
        "updatedAt": 5
    }"#;

    fn sample(address: &str) -> SubscribedCard {
        SubscribedCard {
            address: address.into(),
            relay_url: "http://relay.test".into(),
            display_name: "财报助手".into(),
            specialty_tags: vec!["finance".into()],
            min_redaction_level: "R2".into(),
            auto_answer: "off".into(),
            card_public_key: "pk".into(),
            subscribed_at: 100,
            refreshed_at: 100,
        }
    }

    #[test]
    fn local_layer_upsert_unsubscribe_not_subscribed() {
        with_temp_store(|_dir| {
            // 起始为空。
            assert!(list().is_empty());

            // 幂等 upsert：同 address 覆盖不重复，保留首次订阅时间。
            write_all(&[sample("alice/a")]).unwrap();
            let mut updated = sample("alice/a");
            updated.display_name = "新名字".into();
            updated.refreshed_at = 200;
            let mut cards = read_all().unwrap();
            cards[0] = updated;
            write_all(&cards).unwrap();
            let all = list();
            assert_eq!(all.len(), 1, "同 address 覆盖不重复");
            assert_eq!(all[0].display_name, "新名字");
            assert_eq!(all[0].subscribed_at, 100, "重复订阅保留首次订阅时间");

            // 退订：删除条目；未订阅再退 → NOT_SUBSCRIBED。
            write_all(&[sample("alice/a"), sample("bob/b")]).unwrap();
            unsubscribe("alice/a").unwrap();
            assert_eq!(
                list().iter().map(|c| c.address.clone()).collect::<Vec<_>>(),
                vec!["bob/b"]
            );
            assert!(unsubscribe("alice/a").unwrap_err().contains("NOT_SUBSCRIBED"));
        });
    }

    #[test]
    fn split_address_rejects_malformed() {
        assert!(split_address("noslash").is_err());
        assert!(split_address("/slug").is_err());
        assert!(split_address("user/").is_err());
        assert!(split_address("a/b/c").is_err());
        assert_eq!(split_address("alice/caibao").unwrap(), ("alice".into(), "caibao".into()));
    }

    #[test]
    fn snapshot_fields_map_from_directory_card() {
        let card = serde_json::from_value::<DirectoryCard>(serde_json::json!({
            "address": "alice/caibao",
            "username": "alice",
            "agentSlug": "caibao",
            "displayName": "财报解读助手",
            "visibility": "public",
            "card": {
                "specialtyTags": ["finance/财报"],
                "minRedactionLevel": "R2",
                "autoAnswer": "R3",
                "publicKey": "pk1"
            },
            "updatedAt": 5
        }))
        .unwrap();
        let snapshot = snapshot_from_card(&card, "http://relay.test", 42, None);
        assert_eq!(snapshot.specialty_tags, vec!["finance/财报".to_string()]);
        assert_eq!(snapshot.min_redaction_level, "R2");
        assert_eq!(snapshot.auto_answer, "R3");
        assert_eq!(snapshot.card_public_key, "pk1");
        assert_eq!((snapshot.subscribed_at, snapshot.refreshed_at), (42, 42));
    }

    #[test]
    fn list_returns_empty_on_corrupt_store() {
        with_temp_store(|dir| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("subscriptions.json"), b"{not json").unwrap();
            assert!(list().is_empty(), "损坏存储降级为空列表");
        });
    }

    #[test]
    fn subscribe_rejects_invalid_address_before_network() {
        with_temp_store(|_dir| {
            let relay = spawn_mock_relay("200 OK", CARD_JSON);
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let error = runtime
                .block_on(subscribe("no-slash", Some(&relay)))
                .unwrap_err();
            assert!(error.contains("SUBSCRIPTION_ADDRESS_INVALID"), "{error}");
            // 未落任何条目。
            assert!(list().is_empty());
        });
    }

    #[test]
    fn refresh_reports_not_subscribed_before_network() {
        with_temp_store(|_dir| {
            let relay = spawn_mock_relay("200 OK", CARD_JSON);
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let error = runtime
                .block_on(refresh("alice/caibao"))
                .unwrap_err();
            assert!(error.contains("NOT_SUBSCRIBED"), "{error}");
            let _ = relay;
        });
    }

    #[tokio::test]
    async fn subscribe_fetches_and_stores_snapshot_with_explicit_relay() {
        let dir = tempfile::tempdir().unwrap();
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = Some(dir.path().join("subscriptions.json")));
        let relay = spawn_mock_relay("200 OK", CARD_JSON);

        let snapshot = subscribe("alice/caibao", Some(&relay)).await.unwrap();
        assert_eq!(snapshot.address, "alice/caibao");
        assert_eq!(snapshot.relay_url, relay, "显式 relayUrl 优先，不依赖本地账号");
        assert_eq!(snapshot.display_name, "财报解读助手");
        assert_eq!(snapshot.min_redaction_level, "R2");
        assert_eq!(snapshot.card_public_key, "pk1");
        assert_eq!(list().len(), 1);
    }

    #[tokio::test]
    async fn subscribe_repeats_keep_first_subscribed_at() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("subscriptions.json");
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = Some(store.clone()));
        // 已有订阅（旧快照）→ 再订阅（远端新内容）= 覆盖但保留首次时间。
        write_all(&[sample("alice/caibao")]).unwrap();
        let relay = spawn_mock_relay("200 OK", CARD_JSON);

        let snapshot = subscribe("alice/caibao", Some(&relay)).await.unwrap();
        assert_eq!(snapshot.subscribed_at, 100, "重复订阅保留首次订阅时间");
        assert_eq!(snapshot.display_name, "财报解读助手", "内容更新为远端最新");
        assert_eq!(list().len(), 1, "幂等不重复");
    }

    #[tokio::test]
    async fn refresh_not_found_keeps_snapshot_untouched() {
        let dir = tempfile::tempdir().unwrap();
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = Some(dir.path().join("subscriptions.json")));
        let mut current = sample("alice/caibao");
        // 快照的 relay 指向 404 的 mock：远端 private/不存在语义（T9）。
        current.relay_url = spawn_mock_relay("404 Not Found", "{}");
        write_all(&[current]).unwrap();

        let error = refresh("alice/caibao").await.unwrap_err();
        assert!(error.contains("CARD_FETCH_REJECTED:404"), "{error}");
        // 快照不动：地址与字段保持原样。
        let after = list();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].address, "alice/caibao");
        assert_eq!(after[0].refreshed_at, 100);
    }

    #[tokio::test]
    async fn refresh_unreachable_keeps_snapshot_untouched() {
        let dir = tempfile::tempdir().unwrap();
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = Some(dir.path().join("subscriptions.json")));
        let mut existing = sample("alice/caibao");
        // 127.0.0.1:1 保留端口，连接即拒（T9 网络失败路径）。
        existing.relay_url = "http://127.0.0.1:1".into();
        write_all(&[existing]).unwrap();

        let error = refresh("alice/caibao").await.unwrap_err();
        assert!(error.contains("CARD_FETCH_FAILED"), "{error}");
        assert_eq!(list().len(), 1, "网络失败快照不动");
    }

    #[tokio::test]
    async fn refresh_updates_snapshot_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("subscriptions.json");
        super::TEST_PATH.with(|cell| *cell.borrow_mut() = Some(store));
        let mut existing = sample("alice/caibao");
        let relay = spawn_mock_relay("200 OK", CARD_JSON);
        existing.relay_url = relay;
        write_all(&[existing]).unwrap();

        let snapshot = refresh("alice/caibao").await.unwrap();
        assert_eq!(snapshot.display_name, "财报解读助手", "内容刷新为远端最新");
        assert_eq!(snapshot.subscribed_at, 100, "订阅时间保留");
        assert!(snapshot.refreshed_at >= 100);
    }
}
