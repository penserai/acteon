//! JWKS-style discovery endpoint for action signing keys.
//!
//! Publishes the active verifier set so clients can fetch the
//! current keyring at runtime instead of pinning it in their
//! configuration. Modeled loosely on JWKS (`/.well-known/jwks.json`)
//! but uses a simpler, signing-specific shape since Acteon only
//! supports Ed25519 today.
//!
//! The endpoint is **public** (no authentication) — only public
//! keys are returned, never private key material. Operators who
//! want to keep their verifier set private can disable the
//! endpoint by leaving `signing.enabled` off, in which case the
//! response is an empty list.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Serialize;
use utoipa::ToSchema;

use super::AppState;

/// One verifying key in the active set.
#[derive(Debug, Serialize, ToSchema)]
pub struct SigningKeyEntry {
    /// Logical signer identifier (e.g. `ci-bot`, `deploy-service`).
    pub signer_id: String,
    /// Key identifier within the signer. When the same `signer_id`
    /// has multiple keys (during a rotation window), `kid` selects
    /// the specific one.
    pub kid: String,
    /// Cryptographic algorithm — currently always `Ed25519`.
    pub algorithm: String,
    /// Raw 32-byte public key, base64-encoded.
    pub public_key: String,
    /// Tenant scopes this key is authorized to sign for. `["*"]`
    /// means all tenants.
    pub tenants: Vec<String>,
    /// Namespace scopes this key is authorized to sign for. `["*"]`
    /// means all namespaces.
    pub namespaces: Vec<String>,
}

/// Response from `GET /.well-known/acteon-signing-keys`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SigningKeysResponse {
    /// Every active signing key. Empty when signing is disabled.
    pub keys: Vec<SigningKeyEntry>,
    /// Number of entries. Convenience for clients that just want a count.
    pub count: usize,
}

/// `GET /.well-known/acteon-signing-keys` — JWKS-style discovery.
///
/// Returns the active set of public verifying keys so clients can:
///
/// - Fetch the keyring at runtime instead of pinning it at deploy
///   time (the original use case for this endpoint).
/// - Independently verify dispatched actions outside of the gateway
///   without needing access to the server's TOML config.
/// - Detect a key rotation in progress: when a signer has more than
///   one entry in the response, the operator is staging a rotation
///   and clients should start sending the new `kid`.
#[utoipa::path(
    get,
    path = "/.well-known/acteon-signing-keys",
    tag = "Signing",
    summary = "Discover active signing keys",
    description = "Public JWKS-style endpoint that lists every active Ed25519 verifying key in the server's keyring, including the per-key (signer_id, kid) pair and the tenant + namespace scopes the key is authorized for. Always returns 200 — when signing is disabled or no keys are configured, the `keys` array is empty.",
    responses(
        (status = 200, description = "Active signing key set", body = SigningKeysResponse)
    )
)]
#[allow(clippy::unused_async)]
pub async fn discover_signing_keys(State(state): State<AppState>) -> impl IntoResponse {
    let keys: Vec<SigningKeyEntry> = match &state.signature_verifier {
        Some(verifier) => verifier
            .iter_keys()
            .map(|key| {
                let (tenants, namespaces) = verifier
                    .scope_for(key.signer_id())
                    .map_or_else(
                        || (vec!["*".into()], vec!["*".into()]),
                        |s| (s.tenants.clone(), s.namespaces.clone()),
                    );
                SigningKeyEntry {
                    signer_id: key.signer_id().to_owned(),
                    kid: key.kid().to_owned(),
                    algorithm: "Ed25519".into(),
                    public_key: B64.encode(key.public_key_bytes()),
                    tenants,
                    namespaces,
                }
            })
            .collect(),
        None => Vec::new(),
    };
    let count = keys.len();
    (StatusCode::OK, Json(SigningKeysResponse { keys, count }))
}
