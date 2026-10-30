//! FD-11 P2-9 盲转加密：NaCl box（X25519 ECDH + XSalsa20-Poly1305）。
//!
//! 密钥派生**不引入新密钥体系**：账号 Ed25519 密钥经 `to_montgomery`
//! 换算为 X25519（NaCl 生态标准等价：ed25519 与 curve25519 私钥可互换），
//! 目录卡公钥即加密对端公钥。边界（F5 §6）：仅 `payload.text` 加密，
//! disclosure 与 provenance 恒明文（计量与跳数执法依赖）。
//!
//! 时序纪律：加密发生在确认门通过时、签名之前——签名对含密文的
//! canonical_bytes 计算，密文随留档固化（byte-equal 不变）。

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use crypto_box::aead::{Aead, AeadCore};
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::Digest as _;

fn x25519_public(ed25519_public_b64: &str) -> Result<crypto_box::PublicKey, String> {
    let bytes = BASE64
        .decode(ed25519_public_b64.trim())
        .map_err(|_| "E2E_KEY_DECODE_FAILED".to_string())?;
    let bytes: &[u8; 32] =
        bytes.as_slice().try_into().map_err(|_| "E2E_KEY_INVALID".to_string())?;
    let verifying_key =
        VerifyingKey::from_bytes(bytes).map_err(|_| "E2E_KEY_INVALID".to_string())?;
    Ok(crypto_box::PublicKey::from(verifying_key.to_montgomery().to_bytes()))
}

/// ed25519 → x25519 私钥（NaCl 标准：clamp(sha512(seed)[..32])，
/// 与 libsodium `crypto_sign_ed25519_sk_to_curve25519` 互操作）。
fn x25519_secret(signing_key: &SigningKey) -> crypto_box::SecretKey {
    let mut scalar = [0u8; 32];
    scalar.copy_from_slice(&sha2::Sha512::digest(&signing_key.to_bytes())[..32]);
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
    crypto_box::SecretKey::from(scalar)
}

/// 加密明文 → base64(NaCl box 密文，含随机 nonce 前缀)。
pub fn seal(
    plaintext: &str,
    recipient_ed25519_public_b64: &str,
    my_signing_key: &SigningKey,
) -> Result<String, String> {
    let recipient = x25519_public(recipient_ed25519_public_b64)?;
    let nonce = crypto_box::SalsaBox::generate_nonce(&mut rand::rngs::OsRng);
    let encrypted = crypto_box::SalsaBox::new(&recipient, &x25519_secret(my_signing_key))
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| "E2E_ENCRYPT_FAILED".to_string())?;
    // wire 形态：nonce(24B) || ciphertext+tag（AEAD encrypt 不自带 nonce）。
    let mut sealed = nonce.as_slice().to_vec();
    sealed.extend_from_slice(&encrypted);
    Ok(BASE64.encode(sealed))
}

/// 解密 base64(NaCl box 密文)。失败返回 Err（留档由调用方标记）。
pub fn open(
    cipher_b64: &str,
    sender_ed25519_public_b64: &str,
    my_signing_key: &SigningKey,
) -> Result<String, String> {
    let cipher = BASE64
        .decode(cipher_b64.trim())
        .map_err(|_| "E2E_CIPHER_DECODE_FAILED".to_string())?;
    let sender = x25519_public(sender_ed25519_public_b64)?;
    let nonce_bytes: &[u8; 24] = cipher
        .get(..24)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| "E2E_CIPHER_INVALID".to_string())?;
    let plaintext = crypto_box::SalsaBox::new(&sender, &x25519_secret(my_signing_key))
        .decrypt(&crypto_box::Nonce::from(*nonce_bytes), &cipher[24..])
        .map_err(|_| "E2E_DECRYPT_FAILED".to_string())?;
    String::from_utf8(plaintext).map_err(|_| "E2E_PLAINTEXT_INVALID".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

    fn keypair(seed: u8) -> SigningKey {
        let mut seed_bytes = [seed; 32];
        seed_bytes[0] = seed;
        SigningKey::from_bytes(&seed_bytes)
    }

    #[test]
    fn seal_open_roundtrip() {
        let alice = keypair(1);
        let bob = keypair(2);
        let alice_public = BASE64.encode(alice.verifying_key().as_bytes());
        let bob_public = BASE64.encode(bob.verifying_key().as_bytes());

        // alice → bob：alice 用 bob 公钥加密；bob 用 alice 公钥解密。
        let cipher = seal("咨询内容", &bob_public, &alice).unwrap();
        let plaintext = open(&cipher, &alice_public, &bob).unwrap();
        assert_eq!(plaintext, "咨询内容");

        // 非目标密钥解不开（错向）。
        let mallory = keypair(3);
        assert!(open(&cipher, &alice_public, &mallory).is_err());
        // 篡改密文解不开。
        let tampered = BASE64.encode([0u8; 40]);
        assert!(open(&tampered, &alice_public, &bob).is_err());
    }

    #[test]
    fn signature_key_usable_as_identity() {
        // 挑战签名同源密钥：加解密与签名共存互不影响。
        use ed25519_dalek::Verifier as _;

        let alice = keypair(7);
        let bob = keypair(9);
        let sig = alice.sign(b"challenge");
        assert!(alice.verifying_key().verify(b"challenge", &sig).is_ok());
        let cipher = seal("x", &BASE64.encode(bob.verifying_key().as_bytes()), &alice).unwrap();
        assert!(open(&cipher, &BASE64.encode(alice.verifying_key().as_bytes()), &bob).is_ok());
    }
}
