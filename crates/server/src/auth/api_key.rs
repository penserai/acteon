use std::collections::HashMap;

use sha2::{Digest, Sha256};

use super::config::{ApiKeyConfig, Grant};
use super::crypto::ExposeSecret;
use super::identity::CallerIdentity;
use super::role::Role;

/// An entry in the API key lookup table.
#[derive(Debug, Clone)]
pub struct ApiKeyEntry {
    pub name: String,
    pub principal: Option<acteon_core::PrincipalIdentity>,
    pub role: Role,
    pub grants: Vec<Grant>,
}

/// Build an in-memory lookup from `sha256_hex(raw_key) -> ApiKeyEntry`.
///
/// The config stores pre-computed SHA-256 hashes of the raw keys.
pub fn build_api_key_table(
    configs: &[ApiKeyConfig],
) -> Result<HashMap<String, ApiKeyEntry>, String> {
    let mut map = HashMap::new();
    for cfg in configs {
        let role = Role::from_str_loose(&cfg.role)
            .ok_or_else(|| format!("invalid role for API key '{}'", cfg.name))?;
        if map.contains_key(cfg.key_hash.expose_secret()) {
            return Err("duplicate API key hash".into());
        }
        map.insert(
            cfg.key_hash.expose_secret().to_string(),
            ApiKeyEntry {
                name: cfg.name.clone(),
                principal: cfg.principal.clone(),
                role,
                grants: cfg.grants.clone(),
            },
        );
    }
    Ok(map)
}

/// Hash a raw API key to the lookup format (lowercase hex SHA-256).
pub fn hash_api_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Look up an API key and return a `CallerIdentity` if found.
#[allow(clippy::implicit_hasher)]
pub fn authenticate_api_key(
    raw_key: &str,
    table: &HashMap<String, ApiKeyEntry>,
) -> Option<CallerIdentity> {
    let hash = hash_api_key(raw_key);
    table.get(&hash).map(|entry| CallerIdentity {
        id: entry.name.clone(),
        principal: entry.principal.clone(),
        role: entry.role,
        grants: entry.grants.clone(),
        auth_method: "api_key".to_owned(),
    })
}
