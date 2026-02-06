use serde::{Deserialize, Serialize};

use super::crypto::SecretString;

/// Top-level schema for `auth.toml`.
#[derive(Debug, Deserialize)]
pub struct AuthFileConfig {
    pub settings: AuthSettings,
    #[serde(default)]
    pub users: Vec<UserConfig>,
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,
}

/// Global auth settings.
#[derive(Debug, Deserialize)]
pub struct AuthSettings {
    /// JWT signing secret (may be `ENC[...]` before decryption).
    ///
    /// Wrapped in [`SecretString`] so it is redacted in logs.
    pub jwt_secret: SecretString,
    /// JWT token lifetime in seconds.
    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry_seconds: u64,
}

fn default_jwt_expiry() -> u64 {
    3600
}

/// A user principal that authenticates via username/password and receives a JWT.
#[derive(Debug, Deserialize)]
pub struct UserConfig {
    pub username: String,
    /// Argon2 password hash (may be `ENC[...]` before decryption).
    ///
    /// Wrapped in [`SecretString`] so it is redacted in logs.
    pub password_hash: SecretString,
    /// Role: `"admin"`, `"operator"`, or `"viewer"`.
    pub role: String,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

/// A resource-level grant scoped to tenants, namespaces, and action types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// Tenant identifiers, or `["*"]` for all.
    pub tenants: Vec<String>,
    /// Namespace identifiers, or `["*"]` for all.
    pub namespaces: Vec<String>,
    /// Action type identifiers, or `["*"]` for all.
    pub actions: Vec<String>,
}

/// An API key principal that authenticates via `X-API-Key` header.
#[derive(Debug, Deserialize)]
pub struct ApiKeyConfig {
    pub name: String,
    /// SHA-256 hash of the raw key (may be `ENC[...]` before decryption).
    ///
    /// Wrapped in [`SecretString`] so it is redacted in logs.
    pub key_hash: SecretString,
    /// Role: `"admin"`, `"operator"`, or `"viewer"`.
    pub role: String,
    #[serde(default)]
    pub grants: Vec<Grant>,
}
