pub use acteon_crypto::{
    CryptoError, ExposeSecret, MasterKey, SecretString, decrypt_value, encrypt_value, is_encrypted,
    parse_master_key,
};

/// Decrypt all `ENC[...]` values in an [`super::config::AuthFileConfig`] in place.
///
/// Fields are already [`SecretString`], so decrypted values stay wrapped —
/// no secrets are exposed during this operation.
pub fn decrypt_auth_config(
    config: &mut super::config::AuthFileConfig,
    master_key: &MasterKey,
) -> Result<(), String> {
    config.settings.jwt_secret =
        decrypt_value(config.settings.jwt_secret.expose_secret(), master_key)
            .map_err(|e| e.to_string())?;
    for user in &mut config.users {
        user.password_hash = decrypt_value(user.password_hash.expose_secret(), master_key)
            .map_err(|e| e.to_string())?;
    }
    for key in &mut config.api_keys {
        key.key_hash =
            decrypt_value(key.key_hash.expose_secret(), master_key).map_err(|e| e.to_string())?;
    }
    Ok(())
}
