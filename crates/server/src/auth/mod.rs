pub mod api_key;
pub mod config;
pub mod crypto;
pub mod identity;
pub mod jwt;
pub mod middleware;
pub mod password;
pub mod role;
pub mod watcher;

use std::collections::HashMap;
use std::sync::Arc;

use acteon_state::StateStore;
use tokio::sync::RwLock;
use tracing::info;

use self::api_key::{ApiKeyEntry, authenticate_api_key, build_api_key_table};
use self::config::{AuthFileConfig, Grant};
use self::crypto::{ExposeSecret, SecretString};
use self::identity::CallerIdentity;
use self::jwt::JwtManager;
use self::role::Role;

/// In-memory user entry for fast lookup.
#[derive(Debug, Clone)]
pub struct UserEntry {
    pub password_hash: SecretString,
    pub role: Role,
    pub grants: Vec<Grant>,
}

/// Internal mutable state that can be hot-reloaded.
struct AuthTables {
    /// Username to `UserEntry` lookup table.
    users: HashMap<String, UserEntry>,
    /// SHA-256 hex hash to `ApiKeyEntry` lookup table.
    api_keys: HashMap<String, ApiKeyEntry>,
}

/// Central auth provider built once at startup from the decrypted `auth.toml`.
///
/// Supports hot-reloading of users and API keys via [`reload`](Self::reload).
/// JWT settings (secret, expiry) are immutable after creation to avoid
/// invalidating existing sessions.
pub struct AuthProvider {
    jwt_manager: JwtManager,
    state_store: Arc<dyn StateStore>,
    /// Hot-reloadable auth tables protected by `RwLock`.
    tables: RwLock<AuthTables>,
}

impl AuthProvider {
    /// Build the auth provider from a decrypted config and a state store reference.
    pub fn new(config: &AuthFileConfig, state_store: Arc<dyn StateStore>) -> Result<Self, String> {
        let jwt_manager = JwtManager::new(
            config.settings.jwt_secret.expose_secret(),
            config.settings.jwt_expiry_seconds,
        );

        let tables = Self::build_tables(config)?;

        Ok(Self {
            jwt_manager,
            state_store,
            tables: RwLock::new(tables),
        })
    }

    /// Build the internal lookup tables from configuration.
    fn build_tables(config: &AuthFileConfig) -> Result<AuthTables, String> {
        let mut users = HashMap::new();
        for u in &config.users {
            let role = Role::from_str_loose(&u.role)
                .ok_or_else(|| format!("invalid role '{}' for user '{}'", u.role, u.username))?;
            users.insert(
                u.username.clone(),
                UserEntry {
                    password_hash: u.password_hash.clone(),
                    role,
                    grants: u.grants.clone(),
                },
            );
        }

        let api_keys = build_api_key_table(&config.api_keys);

        Ok(AuthTables { users, api_keys })
    }

    /// Hot-reload users and API keys from a new configuration.
    ///
    /// This atomically swaps the internal lookup tables. JWT settings
    /// (secret, expiry) are not reloaded to avoid invalidating existing tokens.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration contains invalid roles.
    pub async fn reload(&self, config: &AuthFileConfig) -> Result<(), String> {
        let new_tables = Self::build_tables(config)?;

        let user_count = new_tables.users.len();
        let key_count = new_tables.api_keys.len();

        let mut tables = self.tables.write().await;
        *tables = new_tables;

        info!(
            users = user_count,
            api_keys = key_count,
            "auth tables reloaded"
        );
        Ok(())
    }

    /// Authenticate a user by username/password and issue a JWT.
    pub async fn login(
        &self,
        username: &str,
        password_candidate: &str,
    ) -> Result<(String, u64), String> {
        let tables = self.tables.read().await;
        let user = tables
            .users
            .get(username)
            .ok_or_else(|| "invalid credentials".to_owned())?;

        if !password::verify_password(user.password_hash.expose_secret(), password_candidate) {
            return Err("invalid credentials".to_owned());
        }

        let identity = CallerIdentity {
            id: username.to_owned(),
            role: user.role,
            grants: user.grants.clone(),
            auth_method: "jwt".to_owned(),
        };

        // Drop the read lock before issuing the token (which may also need state access).
        drop(tables);

        self.jwt_manager
            .issue_token(&identity, &self.state_store)
            .await
    }

    /// Validate a JWT token and return the caller identity.
    pub async fn validate_jwt(&self, token: &str) -> Result<CallerIdentity, String> {
        self.jwt_manager
            .validate_token(token, &self.state_store)
            .await
    }

    /// Revoke a JWT token (logout).
    pub async fn revoke_jwt(&self, token: &str) -> Result<(), String> {
        self.jwt_manager
            .revoke_token(token, &self.state_store)
            .await?;
        Ok(())
    }

    /// Authenticate an API key and return the caller identity.
    pub async fn authenticate_api_key(&self, raw_key: &str) -> Option<CallerIdentity> {
        let tables = self.tables.read().await;
        authenticate_api_key(raw_key, &tables.api_keys)
    }
}
