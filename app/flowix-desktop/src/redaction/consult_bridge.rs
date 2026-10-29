//! FD-12 M2：`consult_remote` 工具的桌面桥接端点。
//!
//! DSH 会话的 MCP 工具（`flowix-cli mcp` 的 consult_remote）经本回环 HTTP
//! 端点回到桌面进程执行：预授权自动发起链（`consult_auto_send_core`）
//! 深度依赖 AppState（access 配置 / memo_file 锁 / 蒸馏 LLM），CLI 子进程
//! 无法复用。端口与 Bearer token 在启动时随机生成，经 host_registry 的
//! env 白名单注入 dsh-appserver → mcp launcher 透传 → flowix-cli。
//!
//! 安全边界：仅绑 127.0.0.1（不触发防火墙提示）；每进程随机 token；
//! 单端点 POST /consult；无 keep-alive（每连接一请求）。

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::commands::redaction::{consult_auto_send_core, AutoSendResult};
use crate::redaction::consult_registry;

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

struct BridgeInfo {
    port: u16,
    token: String,
}

static BRIDGE: std::sync::OnceLock<BridgeInfo> = std::sync::OnceLock::new();

/// 桥接端点 env（供 host_registry 注入 dsh-appserver 子进程）。
/// 桥未启动时返回空——CLI 工具会以明确错误提示桌面不在运行。
pub fn bridge_env_vars() -> Vec<(&'static str, String)> {
    match BRIDGE.get() {
        Some(info) => vec![
            ("FLOWIX_CONSULT_BRIDGE_PORT", info.port.to_string()),
            ("FLOWIX_CONSULT_BRIDGE_TOKEN", info.token.clone()),
        ],
        None => Vec::new(),
    }
}

/// 幂等启动桥接端点（bootstrap 阶段调用，先于任何 DSH 会话 spawn）。
/// 失败只留痕不阻塞启动：工具调用会收到「桌面桥不可用」的明确错误。
pub fn ensure_started(app: tauri::AppHandle) {
    if BRIDGE.get().is_some() {
        return;
    }
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!("consult bridge bind failed: {error}");
            return;
        }
    };
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(error) => {
            tracing::warn!("consult bridge local_addr failed: {error}");
            return;
        }
    };
    let token = format!("{:032x}", rand::random::<u128>());
    if BRIDGE
        .set(BridgeInfo {
            port,
            token: token.clone(),
        })
        .is_err()
    {
        return; // 并发启动竞争，先到者生效。
    }
    tracing::info!(port, "consult bridge listening on loopback");
    tauri::async_runtime::spawn(async move {
        // from_std 注册 reactor，必须在 tokio runtime 上下文内调用
        //（bootstrap 主线程同步调用会 panic：no reactor running）。
        let listener = match TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!("consult bridge to tokio failed: {error}");
                return;
            }
        };
        let app = app.clone();
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    // 一期内联处理（低频：每次工具调用一个连接）；嵌套
                    // spawn（tokio/tauri::async_runtime 均试）在本环境
                    // 不执行，待查后再放开并发。
                    handle_connection(stream, &app, &token).await;
                }
                Err(error) => {
                    tracing::warn!("consult bridge accept failed: {error}");
                }
            }
        }
    });
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsultBridgeRequest {
    /// 笔记本根（DSH 会话 cwd 经 CLI 透传）；桌面侧 resolve_notebook 校验。
    cwd: String,
    question: String,
    #[serde(default)]
    scope_paths: Vec<String>,
    target_level: String,
    target_agent_id: String,
    session_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsultBridgeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    distill_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    byte_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// 解析后的极简 HTTP 请求（仅支持单请求、无 keep-alive）。
#[derive(Debug, PartialEq)]
pub(crate) struct ParsedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) authorization: Option<String>,
    pub(crate) body: Vec<u8>,
}

/// 从已读字节中解析 `POST /path HTTP/1.1` 请求。
/// 返回 `Ok(None)` = 头部未读完（继续读）；`Err` = 协议错误。
pub(crate) fn parse_request(buffer: &[u8]) -> Result<Option<ParsedRequest>, String> {
    let Some(header_end) = find_subslice(buffer, b"\r\n\r\n") else {
        return Ok(None);
    };
    if header_end > MAX_REQUEST_BYTES {
        return Err("REQUEST_TOO_LARGE".to_string());
    }
    let header = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = header.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || path.is_empty() {
        return Err("MALFORMED_REQUEST_LINE".to_string());
    }
    let mut authorization = None;
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "authorization" {
            authorization = Some(value.to_string());
        } else if name == "content-length" {
            content_length = value
                .parse()
                .map_err(|_| "INVALID_CONTENT_LENGTH".to_string())?;
        }
    }
    if header_end + 4 + content_length > MAX_REQUEST_BYTES {
        return Err("REQUEST_TOO_LARGE".to_string());
    }
    if buffer.len() < header_end + 4 + content_length {
        return Ok(None); // body 未读完。
    }
    let body = buffer[header_end + 4..header_end + 4 + content_length].to_vec();
    Ok(Some(ParsedRequest {
        method,
        path,
        authorization,
        body,
    }))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn handle_connection(mut stream: TcpStream, app: &tauri::AppHandle, token: &str) {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    // 头部 + body 读取窗口：蒸馏外的握手不应久等。
    let parsed = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(30), stream.read(&mut chunk))
            .await
        {
            Ok(Ok(0)) => return, // 对端关闭。
            Ok(Ok(n)) => {
                buffer.extend_from_slice(&chunk[..n]);
                match parse_request(&buffer) {
                    Ok(Some(parsed)) => break parsed,
                    Ok(None) => continue,
                    Err(error) => {
                        let _ = write_response(&mut stream, 400, &format!("{{\"ok\":false,\"error\":\"{error}\"}}")).await;
                        return;
                    }
                }
            }
            Ok(Err(error)) => {
                tracing::warn!("consult bridge read failed: {error}");
                return;
            }
            Err(_) => return, // 读超时，静默断开。
        }
    };
    if parsed.path != "/consult" && parsed.path != "/targets" {
        let _ = write_response(&mut stream, 404, "{\"ok\":false,\"error\":\"NOT_FOUND\"}").await;
        return;
    }
    let authorized = parsed
        .authorization
        .as_deref()
        .is_some_and(|value| value == format!("Bearer {token}"));
    if !authorized {
        let _ = write_response(&mut stream, 403, "{\"ok\":false,\"error\":\"UNAUTHORIZED\"}").await;
        return;
    }
    // FD-12 M2 补：目标枚举（LLM 先列后选——会话内无法得知本机 agent 地址）。
    if parsed.method == "GET" && parsed.path == "/targets" {
        let body = serde_json::to_string(&targets_snapshot()).unwrap_or_default();
        let _ = write_response(&mut stream, 200, &body).await;
        return;
    }
    if parsed.method != "POST" {
        let _ = write_response(&mut stream, 405, "{\"ok\":false,\"error\":\"METHOD_NOT_ALLOWED\"}").await;
        return;
    }
    let request: ConsultBridgeRequest = match serde_json::from_slice(&parsed.body) {
        Ok(value) => value,
        Err(error) => {
            let body = ConsultBridgeResponse {
                ok: false,
                receipt_id: None,
                status: None,
                distill_mode: None,
                byte_length: None,
                error: Some(format!("INVALID_BODY:{error}")),
            };
            let _ = write_response(&mut stream, 200, &serde_json::to_string(&body).unwrap_or_default()).await;
            return;
        }
    };
    // FD-12 M2 补：短地址补全（LLM 常只知道 slug——`expert` / `bob/expert`
    // 自动补本地用户名与中继；完整地址原样通过）。
    let resolved_target = resolve_target(&request.target_agent_id);
    let response = match consult_auto_send_core(
        app,
        &request.cwd,
        &request.question,
        &request.scope_paths,
        &request.target_level,
        &resolved_target,
        &request.session_id,
    )
    .await
    {
        Ok(result) => {
            // FD-12 M3：出门成功登记回程关联（receiptId → 发起会话）并
            // 启动应用级 SSE 订阅——回程注入不依赖收件箱面板。
            let thread_id = consult_registry::resolve_active_dsh_thread(app).await;
            consult_registry::register(consult_registry::ConsultLink {
                receipt_id: result.receipt_id.clone(),
                thread_id,
                cwd: request.cwd.clone(),
                target_agent_id: resolved_target.clone(),
                at: chrono::Utc::now().timestamp(),
                settled: false,
            });
            crate::commands::redaction::ensure_push_subscribed(app);
            success_response(&result)
        }
        Err(error) => ConsultBridgeResponse {
            ok: false,
            receipt_id: None,
            status: None,
            distill_mode: None,
            byte_length: None,
            error: Some(error),
        },
    };
    let body = serde_json::to_string(&response).unwrap_or_default();
    let _ = write_response(&mut stream, 200, &body).await;
}

fn success_response(result: &AutoSendResult) -> ConsultBridgeResponse {
    ConsultBridgeResponse {
        ok: true,
        receipt_id: Some(result.receipt_id.clone()),
        status: Some(result.status.clone()),
        distill_mode: Some(result.distill_mode.clone()),
        byte_length: Some(result.byte_length),
        error: None,
    }
}

/// 本机可寻址目标快照（`consult_targets` 工具数据源）：
/// registered_agents 补全账号用户名与中继 → 完整命名空间地址。
fn targets_snapshot() -> serde_json::Value {
    let config = crate::commands::redaction::redaction_config_for_pipeline();
    let account = crate::redaction::account::read_account();
    let relay = if config.relay_url.trim().is_empty() {
        account.as_ref().map(|item| item.relay_url.clone())
    } else {
        Some(config.relay_url.clone())
    };
    let username = account.as_ref().map(|item| item.username.clone());
    let targets: Vec<serde_json::Value> = config
        .registered_agents
        .iter()
        .map(|agent| {
            let namespaced = if agent.contains('/') {
                agent.clone()
            } else {
                username
                    .as_ref()
                    .map(|user| format!("{user}/{agent}"))
                    .unwrap_or_else(|| agent.clone())
            };
            serde_json::json!({
                "agentId": namespaced,
                "address": relay
                    .as_ref()
                    .map(|url| format!("{namespaced}@{url}"))
                    .unwrap_or_else(|| namespaced.clone()),
            })
        })
        // AGENT-MARKET D-6：订阅的外部 agent（快照自带完整地址，不依赖本地账号）。
        .chain(crate::redaction::subscriptions::list().into_iter().map(|sub| {
            serde_json::json!({
                "agentId": sub.address,
                "address": format!("{}@{}", sub.address, sub.relay_url),
            })
        }))
        .collect();
    serde_json::json!({
        "ok": true,
        "targets": targets,
        "hint": "use the `address` field as consult_remote's target; short forms (slug or user/slug) are also accepted and completed locally",
    })
}

/// 短地址补全：`slug` → `user/slug@relay`；`user/slug` → 补 `@relay`；
/// 完整地址原样。缺账号/中继时尽力返回（后续核心链给出明确错误）。
fn resolve_target(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.contains('@') {
        return trimmed.to_string();
    }
    let config = crate::commands::redaction::redaction_config_for_pipeline();
    let account = crate::redaction::account::read_account();
    let relay = if config.relay_url.trim().is_empty() {
        account.as_ref().map(|item| item.relay_url.clone())
    } else {
        Some(config.relay_url.clone())
    };
    let Some(relay) = relay.filter(|url| !url.trim().is_empty()) else {
        return trimmed.to_string();
    };
    let namespaced = if trimmed.contains('/') {
        trimmed.to_string()
    } else {
        match account.as_ref() {
            Some(item) => format!("{}/{}", item.username, trimmed),
            None => return trimmed.to_string(),
        }
    };
    format!("{namespaced}@{relay}")
}

async fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AGENT-MARKET D-6（T10）：订阅的外部 agent 以完整地址并入 consult_targets。
    #[test]
    fn targets_include_subscribed_cards_with_full_address() {
        let dir = tempfile::tempdir().unwrap();
        crate::redaction::subscriptions::test_support::set_test_path(Some(
            dir.path().join("subscriptions.json"),
        ));
        crate::redaction::subscriptions::test_support::set_entries(vec![
            crate::redaction::subscriptions::SubscribedCard {
                address: "alice/caibao".into(),
                relay_url: "http://relay.test".into(),
                display_name: "财报解读助手".into(),
                specialty_tags: vec!["finance".into()],
                min_redaction_level: "R2".into(),
                auto_answer: "off".into(),
                card_public_key: String::new(),
                subscribed_at: 1,
                refreshed_at: 1,
            },
        ]);
        let snapshot = targets_snapshot();
        let targets = snapshot["targets"].as_array().unwrap();
        let found = targets
            .iter()
            .find(|target| target["agentId"] == serde_json::json!("alice/caibao"))
            .expect("subscribed card in targets");
        assert_eq!(
            found["address"],
            serde_json::json!("alice/caibao@http://relay.test")
        );
        // T10 后半：退订后从 targets 消失。
        crate::redaction::subscriptions::unsubscribe("alice/caibao").unwrap();
        let after = targets_snapshot();
        let targets_after = after["targets"].as_array().unwrap();
        assert!(
            !targets_after
                .iter()
                .any(|target| target["agentId"] == serde_json::json!("alice/caibao")),
            "退订后 targets 不再包含该卡"
        );
        crate::redaction::subscriptions::test_support::set_test_path(None);
    }

    #[test]
    fn parse_request_full_roundtrip() {
        let body = r#"{"question":"q"}"#;
        let raw = format!(
            "POST /consult HTTP/1.1\r\nAuthorization: Bearer t\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let parsed = parse_request(raw.as_bytes()).unwrap().unwrap();
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.path, "/consult");
        assert_eq!(parsed.authorization.as_deref(), Some("Bearer t"));
        assert_eq!(parsed.body, body.as_bytes().to_vec());
    }

    #[test]
    fn parse_request_waits_for_body() {
        let head = "POST /consult HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc";
        assert!(parse_request(head.as_bytes()).unwrap().is_none());
    }

    #[test]
    fn parse_request_rejects_bad_length() {
        let raw = "POST /consult HTTP/1.1\r\nContent-Length: abc\r\n\r\n";
        assert!(parse_request(raw.as_bytes()).is_err());
    }

    #[test]
    fn parse_request_without_content_length_has_empty_body() {
        let raw = "POST /consult HTTP/1.1\r\n\r\n";
        let parsed = parse_request(raw.as_bytes()).unwrap().unwrap();
        assert!(parsed.body.is_empty());
        assert!(parsed.authorization.is_none());
    }
}
