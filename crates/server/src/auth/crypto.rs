use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// A 32-byte AES-256 master key for encrypting/decrypting sensitive config values.
pub type MasterKey = [u8; 32];

/// Parse a 32-byte master key from hex or base64.
pub fn parse_master_key(raw: &str) -> Result<[u8; 32], String> {
    let trimmed = raw.trim();
    // Try hex first (64 hex chars = 32 bytes).
    if trimmed.len() == 64
        && let Ok(bytes) = hex::decode(trimmed)
        && bytes.len() == 32
    {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    // Try base64.
    if let Ok(bytes) = B64.decode(trimmed)
        && bytes.len() == 32
    {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    Err("ACTEON_AUTH_KEY must be 32 bytes encoded as 64 hex chars or base64".to_owned())
}

/// If `value` starts with `ENC[AES256-GCM,...]`, decrypt it using the master
/// key. Otherwise return the value unchanged.
pub fn decrypt_value(value: &str, master_key: &[u8; 32]) -> Result<String, String> {
    let trimmed = value.trim();
    if !trimmed.starts_with("ENC[AES256-GCM,") || !trimmed.ends_with(']') {
        return Ok(value.to_owned());
    }

    let inner = &trimmed["ENC[AES256-GCM,".len()..trimmed.len() - 1];
    let mut data_b64 = None;
    let mut iv_b64 = None;
    let mut tag_b64 = None;

    for part in inner.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("data:") {
            data_b64 = Some(v);
        } else if let Some(v) = part.strip_prefix("iv:") {
            iv_b64 = Some(v);
        } else if let Some(v) = part.strip_prefix("tag:") {
            tag_b64 = Some(v);
        }
    }

    let data = B64
        .decode(data_b64.ok_or("missing data field in ENC[...]")?)
        .map_err(|e| format!("invalid base64 in data: {e}"))?;
    let iv = B64
        .decode(iv_b64.ok_or("missing iv field in ENC[...]")?)
        .map_err(|e| format!("invalid base64 in iv: {e}"))?;
    let tag = B64
        .decode(tag_b64.ok_or("missing tag field in ENC[...]")?)
        .map_err(|e| format!("invalid base64 in tag: {e}"))?;

    if iv.len() != 12 {
        return Err(format!("IV must be 12 bytes, got {}", iv.len()));
    }
    if tag.len() != 16 {
        return Err(format!("tag must be 16 bytes, got {}", tag.len()));
    }

    // AES-GCM ciphertext = data || tag
    let mut ciphertext = data;
    ciphertext.extend_from_slice(&tag);

    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| format!("invalid AES key: {e}"))?;
    let nonce = Nonce::from_slice(&iv);

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| "decryption failed (wrong key or corrupted data)".to_owned())?;

    String::from_utf8(plaintext).map_err(|e| format!("decrypted value is not UTF-8: {e}"))
}

/// Encrypt a plaintext string, producing an `ENC[AES256-GCM,...]` marker.
pub fn encrypt_value(plaintext: &str, master_key: &[u8; 32]) -> Result<String, String> {
    use aes_gcm::AeadCore;

    let cipher =
        Aes256Gcm::new_from_slice(master_key).map_err(|e| format!("invalid AES key: {e}"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("encryption failed: {e}"))?;

    // AES-GCM output = ciphertext_data || 16-byte tag
    let (data, tag) = ciphertext.split_at(ciphertext.len() - 16);

    Ok(format!(
        "ENC[AES256-GCM,data:{},iv:{},tag:{}]",
        B64.encode(data),
        B64.encode(nonce.as_slice()),
        B64.encode(tag),
    ))
}

/// Decrypt all `ENC[...]` values in an [`super::config::AuthFileConfig`] in place.
pub fn decrypt_auth_config(
    config: &mut super::config::AuthFileConfig,
    master_key: &[u8; 32],
) -> Result<(), String> {
    config.settings.jwt_secret = decrypt_value(&config.settings.jwt_secret, master_key)?;
    for user in &mut config.users {
        user.password_hash = decrypt_value(&user.password_hash, master_key)?;
    }
    for key in &mut config.api_keys {
        key.key_hash = decrypt_value(&key.key_hash, master_key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let key = [0x42u8; 32];
        let plaintext = "my-secret-value";
        let encrypted = encrypt_value(plaintext, &key).unwrap();
        assert!(encrypted.starts_with("ENC[AES256-GCM,"));
        let decrypted = decrypt_value(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn passthrough_plain_value() {
        let key = [0x42u8; 32];
        let plain = "not-encrypted";
        let result = decrypt_value(plain, &key).unwrap();
        assert_eq!(result, plain);
    }

    #[test]
    fn parse_hex_key() {
        let hex_key = "aa".repeat(32);
        let key = parse_master_key(&hex_key).unwrap();
        assert_eq!(key, [0xaa; 32]);
    }

    #[test]
    fn parse_base64_key() {
        let raw = [0xbbu8; 32];
        let b64 = B64.encode(raw);
        let key = parse_master_key(&b64).unwrap();
        assert_eq!(key, [0xbb; 32]);
    }
}
