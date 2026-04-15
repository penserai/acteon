//! Client-side helpers for the JWKS-style action signing key
//! discovery endpoint.
//!
//! Lets clients fetch the active verifier set at runtime rather than
//! pinning public keys in their configuration. Useful for:
//!
//! - Side-loaded verification (a client wants to confirm an audit
//!   record's signature without trusting a single hardcoded pubkey).
//! - Detecting a key rotation in progress (when a signer has more
//!   than one entry in the response, the operator is staging a
//!   rotation and the client should start sending the new `kid`).

use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// One verifying key in the active set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigningKeyEntry {
    /// Logical signer identifier.
    pub signer_id: String,
    /// Key identifier within the signer.
    pub kid: String,
    /// Cryptographic algorithm — currently always `Ed25519`.
    pub algorithm: String,
    /// Raw 32-byte public key, base64-encoded.
    pub public_key: String,
    /// Tenant scopes this key is authorized for.
    pub tenants: Vec<String>,
    /// Namespace scopes this key is authorized for.
    pub namespaces: Vec<String>,
}

/// Response from the JWKS-style discovery endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigningKeysResponse {
    /// Every active signing key. Empty when signing is disabled
    /// server-side.
    pub keys: Vec<SigningKeyEntry>,
    /// Number of entries (convenience).
    pub count: usize,
}

impl ActeonClient {
    /// Fetch the active set of action signing keys from
    /// `GET /.well-known/acteon-signing-keys`.
    ///
    /// The endpoint is unauthenticated (only public key material is
    /// returned), so this method does not attach the client's auth
    /// token. Returns an empty `keys` list when signing is disabled
    /// on the server.
    pub async fn fetch_signing_keys(&self) -> Result<SigningKeysResponse, Error> {
        let url = format!("{}/.well-known/acteon-signing-keys", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<SigningKeysResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to fetch signing keys: {}", response.status()),
            })
        }
    }
}
