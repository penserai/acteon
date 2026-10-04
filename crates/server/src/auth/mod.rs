pub mod api_key;
pub mod authority;
pub mod config;
pub mod crypto;
pub mod enrollment;
pub mod identity;
pub mod jwt;
pub mod middleware;
pub mod password;
pub mod role;
pub mod route_permissions;
pub mod watcher;

use std::collections::HashMap;
use std::sync::Arc;

use acteon_state::StateStore;
use tokio::sync::RwLock;
use tracing::info;

use self::api_key::{ApiKeyEntry, authenticate_api_key_bound, build_api_key_table};
use self::config::{AuthFileConfig, Grant};
use self::crypto::{ExposeSecret, SecretString};
use self::identity::CallerIdentity;
use self::jwt::JwtManager;
use self::role::Role;

/// In-memory user entry for fast lookup.
#[derive(Debug, Clone)]
pub struct UserEntry {
    pub password_hash: SecretString,
    pub authority_id: Option<String>,
    pub principal: Option<acteon_core::PrincipalIdentity>,
    pub role: Role,
    pub grants: Vec<Grant>,
}

/// Internal mutable state that can be hot-reloaded.
struct AuthTables {
    /// Username to `UserEntry` lookup table.
    users: HashMap<String, UserEntry>,
    /// SHA-256 hex hash to `ApiKeyEntry` lookup table.
    api_keys: HashMap<String, ApiKeyEntry>,
    authority_reference: Option<acteon_governance::configuration::CredentialConfigurationReference>,
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
    authority: Option<Arc<authority::AuthAuthority>>,
    jwt_settings: (SecretString, u64),
}

pub(super) struct AuthenticatedCaller {
    identity: CallerIdentity,
    binding: Option<authority::AuthenticatedConfiguration>,
    credential: Option<enrollment::AuthenticatedCredential>,
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
            authority: None,
            jwt_settings: (
                config.settings.jwt_secret.clone(),
                config.settings.jwt_expiry_seconds,
            ),
        })
    }

    /// Publish a shared authentication epoch before exposing this provider.
    /// Every subsequent credential lookup verifies the same authoritative source.
    /// This does not enable an execution-permit profile.
    pub async fn new_with_authority(
        config: &AuthFileConfig,
        state_store: Arc<dyn StateStore>,
        authority: Arc<authority::AuthAuthority>,
    ) -> Result<Self, String> {
        let mut provider = Self::new(config, state_store)?;
        let reference = authority.publish(config).await?;
        provider.tables.get_mut().authority_reference = Some(reference);
        provider.authority = Some(authority);
        Ok(provider)
    }

    /// Build the internal lookup tables from configuration.
    fn build_tables(config: &AuthFileConfig) -> Result<AuthTables, String> {
        enrollment::validate_enrollments(config)?;
        let mut users = HashMap::new();
        for u in &config.users {
            if users.contains_key(&u.username) {
                return Err("duplicate username".into());
            }
            let role = Role::from_str_loose(&u.role)
                .ok_or_else(|| format!("invalid role '{}' for user '{}'", u.role, u.username))?;
            users.insert(
                u.username.clone(),
                UserEntry {
                    password_hash: u.password_hash.clone(),
                    authority_id: u.authority_id.clone(),
                    principal: u.principal.clone(),
                    role,
                    grants: u.grants.clone(),
                },
            );
        }

        let api_keys = build_api_key_table(&config.api_keys)?;
        let mut principal_kinds = HashMap::new();
        for principal in config
            .users
            .iter()
            .filter_map(|u| u.principal.as_ref())
            .chain(config.api_keys.iter().filter_map(|k| k.principal.as_ref()))
        {
            if let Some(previous) = principal_kinds.insert(principal.id(), principal.kind())
                && previous != principal.kind()
            {
                return Err("principal ID has conflicting kinds".into());
            }
        }

        Ok(AuthTables {
            users,
            api_keys,
            authority_reference: None,
        })
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
        if self.authority.is_some()
            && (self.jwt_settings.0.expose_secret() != config.settings.jwt_secret.expose_secret()
                || self.jwt_settings.1 != config.settings.jwt_expiry_seconds)
        {
            return Err("JWT settings changed; restart with a new authority revision".into());
        }
        let mut new_tables = Self::build_tables(config)?;

        let user_count = new_tables.users.len();
        let key_count = new_tables.api_keys.len();

        // Serialize publication and local installation. Old tables remain on
        // failure, but cannot authenticate once another snapshot is current.
        let mut tables = self.tables.write().await;
        if let Some(authority) = &self.authority {
            new_tables.authority_reference = Some(authority.publish(config).await?);
        }
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
            principal: user.principal.clone(),
            role: user.role,
            grants: user.grants.clone(),
            auth_method: "jwt".to_owned(),
        };

        self.bind_identity(&tables, &identity).await?;
        let authority_id = user.authority_id.clone();

        // Drop the read lock before issuing the token (which may also need state access).
        drop(tables);

        self.jwt_manager
            .issue_token_with_credential(&identity, authority_id.as_deref(), &self.state_store)
            .await
    }

    /// Validate a JWT token and return the caller identity.
    pub async fn validate_jwt(&self, token: &str) -> Result<CallerIdentity, String> {
        Ok(self.validate_jwt_bound(token).await?.identity)
    }

    pub(super) async fn validate_jwt_bound(
        &self,
        token: &str,
    ) -> Result<AuthenticatedCaller, String> {
        let (mut identity, authority_id) = self
            .jwt_manager
            .validate_token_bound(token, &self.state_store)
            .await?;
        // Claims establish authentication, not a permanent authorization snapshot.
        // Reloaded roles/grants and removed users apply to existing sessions.
        let tables = self.tables.read().await;
        let user = tables
            .users
            .get(&identity.id)
            .ok_or_else(|| "user is no longer authorized".to_owned())?;
        // Existing sessions may refresh privileges, but never change actor.
        if identity.principal != user.principal {
            return Err("principal binding changed; login again".into());
        }
        if authority_id != user.authority_id {
            return Err("credential enrollment changed; login again".into());
        }
        let credential =
            enrollment::AuthenticatedCredential::from_identity(authority_id.as_deref(), &identity)?;
        identity.role = user.role;
        identity.grants.clone_from(&user.grants);
        let binding = self.bind_identity(&tables, &identity).await?;
        Ok(AuthenticatedCaller {
            identity,
            binding,
            credential,
        })
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
        self.authenticate_api_key_bound(raw_key)
            .await
            .map(|caller| caller.identity)
    }

    pub(super) async fn authenticate_api_key_bound(
        &self,
        raw_key: &str,
    ) -> Option<AuthenticatedCaller> {
        let tables = self.tables.read().await;
        let (identity, credential) = authenticate_api_key_bound(raw_key, &tables.api_keys)?;
        let binding = self.bind_identity(&tables, &identity).await.ok()?;
        Some(AuthenticatedCaller {
            identity,
            binding,
            credential,
        })
    }

    async fn bind_identity(
        &self,
        tables: &AuthTables,
        identity: &CallerIdentity,
    ) -> Result<Option<authority::AuthenticatedConfiguration>, String> {
        match (&self.authority, &tables.authority_reference) {
            (Some(authority), Some(reference)) => {
                Ok(Some(authority.verify(reference, identity).await?))
            }
            (None, None) => Ok(None),
            _ => Err("authentication authority binding unavailable".into()),
        }
    }
}
