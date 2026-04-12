use serde::Deserialize;

/// Configuration for Ed25519 action signing and verification.
///
/// When `enabled` is true, the server verifies incoming action
/// signatures against the `keyring` and optionally signs
/// server-originated actions (chains, recurring) with `server_key`.
///
/// # Example TOML
///
/// ```toml
/// [signing]
/// enabled = true
/// reject_unsigned = false
/// server_key = "ENC[AES256-GCM,data:...]"
///
/// [[signing.keyring]]
/// signer_id = "ci-bot"
/// public_key = "base64-encoded-ed25519-public-key"
///
/// [[signing.keyring]]
/// signer_id = "deploy-service"
/// public_key = "hex-encoded-ed25519-public-key"
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SigningConfig {
    /// Enable signature verification on incoming dispatches and
    /// signing of server-originated actions. When false, the
    /// `signature` and `signer_id` fields on actions are ignored.
    pub enabled: bool,

    /// When true, reject any dispatch whose action does not carry a
    /// valid `signature` + `signer_id`. When false (the default),
    /// unsigned actions pass through normally.
    pub reject_unsigned: bool,

    /// Ed25519 secret key for signing server-originated actions
    /// (chains, recurring, DLQ replays). Supports `ENC[...]` for
    /// encrypted storage. When absent, server-originated actions are
    /// dispatched unsigned.
    pub server_key: Option<String>,

    /// Signer identity to stamp on server-originated signatures.
    /// Defaults to `"acteon-server"` when `server_key` is set.
    pub server_signer_id: Option<String>,

    /// Set of named public keys trusted for incoming signature
    /// verification.
    #[serde(default)]
    pub keyring: Vec<KeyringEntry>,
}

/// A single entry in the signing keyring.
#[derive(Debug, Deserialize)]
pub struct KeyringEntry {
    /// Unique identifier for this signer. Must match the `signer_id`
    /// field on incoming actions.
    pub signer_id: String,
    /// Ed25519 public key, encoded as hex (64 chars) or base64.
    pub public_key: String,
}
