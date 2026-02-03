use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use acteon_state::{KeyKind, StateKey, StateStore};

use super::config::Grant;
use super::identity::CallerIdentity;
use super::role::Role;

/// JWT claims embedded in issued tokens.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (username).
    pub sub: String,
    /// Unique token ID for revocation tracking.
    pub jti: String,
    /// Role name.
    pub role: String,
    /// Resource grants.
    pub grants: Vec<Grant>,
    /// Expiry (seconds since epoch).
    pub exp: usize,
}

/// Manages JWT issuance and validation with state-store-backed revocation.
pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    expiry_seconds: u64,
}

impl JwtManager {
    pub fn new(secret: &str, expiry_seconds: u64) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(secret.as_bytes()),
            expiry_seconds,
        }
    }

    /// Issue a JWT for the given identity, storing the `jti` in the state store.
    pub async fn issue_token(
        &self,
        identity: &CallerIdentity,
        state_store: &Arc<dyn StateStore>,
    ) -> Result<(String, u64), String> {
        let jti = uuid::Uuid::new_v4().to_string();
        #[allow(clippy::cast_possible_truncation)]
        let exp = jsonwebtoken::get_current_timestamp() as usize + self.expiry_seconds as usize;

        let claims = Claims {
            sub: identity.id.clone(),
            jti: jti.clone(),
            role: identity.role.to_string(),
            grants: identity.grants.clone(),
            exp,
        };

        let token = encode(&Header::default(), &claims, &self.encoding_key)
            .map_err(|e| format!("JWT encoding failed: {e}"))?;

        // Store jti in state store with TTL for automatic cleanup.
        let state_key = StateKey::new("auth", "tokens", KeyKind::Custom("token".to_owned()), &jti);
        state_store
            .set(
                &state_key,
                "1",
                Some(Duration::from_secs(self.expiry_seconds)),
            )
            .await
            .map_err(|e| format!("failed to store token jti: {e}"))?;

        Ok((token, self.expiry_seconds))
    }

    /// Validate a JWT: check signature, expiry, and that the `jti` still exists
    /// in the state store (not revoked).
    pub async fn validate_token(
        &self,
        token: &str,
        state_store: &Arc<dyn StateStore>,
    ) -> Result<CallerIdentity, String> {
        let token_data = decode::<Claims>(token, &self.decoding_key, &Validation::default())
            .map_err(|e| format!("invalid token: {e}"))?;

        let claims = token_data.claims;

        // Check revocation: jti must exist in state store.
        let state_key =
            StateKey::new("auth", "tokens", KeyKind::Custom("token".to_owned()), &claims.jti);
        let exists = state_store
            .get(&state_key)
            .await
            .map_err(|e| format!("state store error: {e}"))?;

        if exists.is_none() {
            return Err("token has been revoked".to_owned());
        }

        let role = Role::from_str_loose(&claims.role)
            .ok_or_else(|| format!("invalid role in token: {}", claims.role))?;

        Ok(CallerIdentity {
            id: claims.sub,
            role,
            grants: claims.grants,
            auth_method: "jwt".to_owned(),
        })
    }

    /// Revoke a token by deleting its `jti` from the state store.
    pub async fn revoke_token(
        &self,
        token: &str,
        state_store: &Arc<dyn StateStore>,
    ) -> Result<String, String> {
        let token_data = decode::<Claims>(token, &self.decoding_key, &Validation::default())
            .map_err(|e| format!("invalid token: {e}"))?;

        let jti = token_data.claims.jti;
        let state_key = StateKey::new("auth", "tokens", KeyKind::Custom("token".to_owned()), &jti);
        state_store
            .delete(&state_key)
            .await
            .map_err(|e| format!("failed to revoke token: {e}"))?;

        Ok(jti)
    }
}
