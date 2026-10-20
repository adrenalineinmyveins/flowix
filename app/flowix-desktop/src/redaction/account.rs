//! FD-11 v2 中继账号：Ed25519 密钥即身份（F5-CROSS-USER-NETWORK §3/§5）。
//!
//! 账号存 `~/.flowix/redaction/account.json`（全局单账号，绑定单中继）：
//! 私钥只在本文件，出站信封签名与登录挑战共用。注册/登录共用一个端点：
//! 新 username 注册即发 token；已有 username 走挑战签名换发。

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use flowix_relay::envelope::{canonical_bytes, WireEnvelope, WireSignature};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayAccount {
    pub username: String,
    pub user_id: String,
    /// 账号级登录态（中继设备令牌；与收件通道的 device_token 分离）。
    pub device_token: String,
    /// base64(标准) 的 32 字节私钥种子——只落本文件，永不出设备。
    signing_key: String,
    /// base64(标准) 的 32 字节公钥（目录卡信任锚同源）。
    pub public_key: String,
    /// 账号归属的中继地址（与 RedactionConfig.relay_url 对齐才可用）。
    pub relay_url: String,
}

/// 回前端的账号视图（不含私钥）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPublicInfo {
    pub username: String,
    pub user_id: String,
    pub public_key: String,
    pub relay_url: String,
}

impl RelayAccount {
    pub fn signing_key(&self) -> Result<SigningKey, String> {
        let bytes = BASE64
            .decode(self.signing_key.trim())
            .map_err(|_| "ACCOUNT_KEY_DECODE_FAILED".to_string())?;
        let bytes: &[u8; 32] =
            bytes.as_slice().try_into().map_err(|_| "ACCOUNT_KEY_INVALID".to_string())?;
        Ok(SigningKey::from_bytes(bytes))
    }

    pub fn public_info(&self) -> AccountPublicInfo {
        AccountPublicInfo {
            username: self.username.clone(),
            user_id: self.user_id.clone(),
            public_key: self.public_key.clone(),
            relay_url: self.relay_url.clone(),
        }
    }

    /// 对信封规范化字节签名（wire v2 `signature` 字段）。
    pub fn sign_envelope(&self, envelope: &WireEnvelope) -> Result<WireSignature, String> {
        let key = self.signing_key()?;
        let signature = key.sign(&canonical_bytes(envelope));
        Ok(WireSignature {
            alg: "ed25519".to_string(),
            key_id: self.public_key.clone(),
            value: BASE64.encode(signature.to_bytes()),
        })
    }
}

/// 验签（入站信任锚 = 目录卡公钥）。
pub fn verify_signature(envelope: &WireEnvelope, public_key_b64: &str) -> bool {
    let Some(signature) = envelope.signature.as_ref() else { return false };
    let key_bytes = match BASE64.decode(public_key_b64.trim()) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let key_bytes: &[u8; 32] = match key_bytes.as_slice().try_into() {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let verifying_key = match VerifyingKey::from_bytes(key_bytes) {
        Ok(key) => key,
        Err(_) => return false,
    };
    let signature_bytes = match BASE64.decode(signature.value.trim()) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let signature = match Signature::from_slice(&signature_bytes) {
        Ok(signature) => signature,
        Err(_) => return false,
    };
    verifying_key.verify(&canonical_bytes(envelope), &signature).is_ok()
}

pub fn account_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction").join("account.json"))
}

pub fn read_account() -> Option<RelayAccount> {
    let path = account_path().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    // 与 config 同源的 BOM 容错。
    serde_json::from_str::<RelayAccount>(raw.trim_start_matches('\u{feff}')).ok()
}

pub fn write_account(account: &RelayAccount) -> Result<(), String> {
    let path = account_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("ACCOUNT_DIR_FAILED: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(account).map_err(|e| format!("ACCOUNT_SERIALIZE_FAILED: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("ACCOUNT_WRITE_FAILED: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("ACCOUNT_WRITE_FAILED: {e}"))?;
    Ok(())
}

pub fn forget_account() -> Result<(), String> {
    let path = account_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("ACCOUNT_FORGET_FAILED: {error}")),
    }
}

fn relay_endpoint(relay_url: &str, path: &str) -> String {
    super::inbound::relay_endpoint(relay_url, path)
}

async fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("ACCOUNT_CLIENT_INIT_FAILED: {e}"))
}

/// 注册或登录：本机已有同中继账号 → 复用私钥走挑战登录；否则生成新密钥
/// 注册（201 即得 token）。username 被他人占用 → `USERNAME_TAKEN`。
pub async fn register_or_login(username: &str, relay_url: &str) -> Result<RelayAccount, String> {
    let username = username.trim().to_lowercase();
    if username.len() < 3
        || username.len() > 32
        || !username.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || username.starts_with('-')
    {
        return Err("USERNAME_INVALID".to_string());
    }
    super::transport::validate_relay_url(relay_url)?;

    let existing = read_account();
    let signing_key = match &existing {
        Some(account) if account.username == username && account.relay_url == relay_url.trim() => {
            account.signing_key()?
        }
        _ => {
            let mut seed = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
            SigningKey::from_bytes(&seed)
        }
    };
    let public_key = BASE64.encode(signing_key.verifying_key().as_bytes());

    let client = http_client().await?;
    let response = client
        .post(relay_endpoint(relay_url, "/v2/accounts"))
        .json(&serde_json::json!({ "username": username, "devicePublicKey": public_key }))
        .send()
        .await
        .map_err(|e| format!("ACCOUNT_REQUEST_FAILED: {e}"))?;
    if response.status() == reqwest::StatusCode::CONFLICT {
        return Err("USERNAME_TAKEN".to_string());
    }
    if !response.status().is_success() {
        return Err(format!("ACCOUNT_REJECTED:{}", response.status().as_u16()));
    }
    let body: serde_json::Value =
        response.json().await.map_err(|e| format!("ACCOUNT_DECODE_FAILED: {e}"))?;
    let user_id = body["userId"].as_str().ok_or("ACCOUNT_RESPONSE_INVALID")?.to_string();
    let device_token = match body["deviceToken"].as_str() {
        Some(token) => token.to_string(),
        None => {
            // 登录挑战：签名换发。
            let challenge = body["challenge"].as_str().ok_or("ACCOUNT_RESPONSE_INVALID")?;
            let signature = BASE64.encode(signing_key.sign(challenge.as_bytes()).to_bytes());
            let response = client
                .post(relay_endpoint(relay_url, "/v2/accounts/challenge"))
                .json(&serde_json::json!({
                    "userId": user_id, "challenge": challenge, "signature": signature,
                }))
                .send()
                .await
                .map_err(|e| format!("ACCOUNT_LOGIN_FAILED: {e}"))?;
            if !response.status().is_success() {
                return Err(format!("ACCOUNT_LOGIN_REJECTED:{}", response.status().as_u16()));
            }
            let body: serde_json::Value =
                response.json().await.map_err(|e| format!("ACCOUNT_DECODE_FAILED: {e}"))?;
            body["deviceToken"].as_str().ok_or("ACCOUNT_RESPONSE_INVALID")?.to_string()
        }
    };
    let account = RelayAccount {
        username,
        user_id,
        device_token,
        signing_key: BASE64.encode(signing_key.to_bytes()),
        public_key,
        relay_url: relay_url.trim().to_string(),
    };
    write_account(&account)?;
    Ok(account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flowix_relay::envelope::{
        AgentRef, WireDisclosure, WirePayload, WireProvenance, ENVELOPE_VERSION,
    };

    fn sample() -> WireEnvelope {
        WireEnvelope {
            envelope: ENVELOPE_VERSION.to_string(),
            from: AgentRef { agent_id: "alice/researcher".into(), display_name: "A".into() },
            to: AgentRef { agent_id: "bob/observer".into(), display_name: "B".into() },
            in_reply_to: None,
            disclosure: WireDisclosure {
                redaction_level: "R2".into(),
                max_hops: 2,
                hops_remaining: 2,
                expires_at: 4_102_444_800,
                allowed_scopes: vec![],
            },
            payload: WirePayload { kind: "consultation".into(), text: "咨询".into() },
            provenance: WireProvenance {
                pipeline_version: "1".into(),
                rule_set_version: "rs".into(),
                used_fallback: false,
                confirmed_by: "user".into(),
                confirmed_at: 1,
                receipt_id: "rct_acc".into(),
            },
            signature: None,
        }
    }

    fn test_account() -> RelayAccount {
        let mut seed = [7u8; 32];
        seed[0] = rand::random::<u8>();
        let key = SigningKey::from_bytes(&seed);
        RelayAccount {
            username: "alice".into(),
            user_id: "usr_x".into(),
            device_token: "dev_x".into(),
            signing_key: BASE64.encode(key.to_bytes()),
            public_key: BASE64.encode(key.verifying_key().as_bytes()),
            relay_url: "https://relay.example".into(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let account = test_account();
        let signature = account.sign_envelope(&sample()).unwrap();
        let mut signed = sample();
        signed.signature = Some(signature);
        // 公钥匹配 → 验签通过；篡改 payload → 失败。
        assert!(verify_signature(&signed, &account.public_key));
        let mut tampered = signed.clone();
        tampered.payload.text = "篡改".into();
        assert!(!verify_signature(&tampered, &account.public_key));
        // 异公钥 → 失败。
        let other = test_account();
        assert!(!verify_signature(&signed, &other.public_key));
    }

    #[test]
    fn signing_key_roundtrip() {
        let account = test_account();
        assert!(account.signing_key().is_ok());
        let corrupted = RelayAccount { signing_key: "!!!".into(), ..test_account() };
        assert!(corrupted.signing_key().is_err());
    }
}
