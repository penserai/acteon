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

    /// When true, reject any dispatch whose action ID has already
    /// been processed (replay protection). Uses the state store to
    /// track seen action IDs with a configurable TTL. Defaults to
    /// false for backward compatibility.
    pub reject_replay: bool,

    /// TTL in seconds for replay-protection entries in the state
    /// store. After this period, a replayed action ID would be
    /// accepted again. Defaults to 86400 (24 hours).
    #[serde(default = "default_replay_ttl")]
    pub replay_ttl_seconds: u64,

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

fn default_replay_ttl() -> u64 {
    86_400 // 24 hours
}

/// A single entry in the signing keyring.
///
/// Multiple entries can share the same `signer_id` as long as their
/// `kid`s differ — that's how key rotation is staged. A typical
/// rotation looks like:
///
/// ```toml
/// # The currently-deployed key.
/// [[signing.keyring]]
/// signer_id = "ci-bot"
/// kid = "k1"
/// public_key = "..."
///
/// # The new key, added before clients flip over.
/// [[signing.keyring]]
/// signer_id = "ci-bot"
/// kid = "k2"
/// public_key = "..."
/// ```
///
/// During the rotation window the verifier accepts signatures from
/// either key (legacy clients that don't stamp a `kid`) or from the
/// specific key matching the action's `kid`. Once all in-flight
/// signed actions have been processed, remove the old entry.
#[derive(Debug, Deserialize)]
pub struct KeyringEntry {
    /// Unique identifier for this signer. Must match the `signer_id`
    /// field on incoming actions.
    pub signer_id: String,
    /// Optional key identifier. Defaults to `"k0"` for backward
    /// compatibility with single-key configs. Must be unique within
    /// a `signer_id`.
    #[serde(default = "default_kid")]
    pub kid: String,
    /// Ed25519 public key, encoded as hex (64 chars) or base64.
    pub public_key: String,
    /// Optional tenant scope. When set, this signer can only sign
    /// actions for the listed tenants. A wildcard `["*"]` (the
    /// default) allows all tenants.
    #[serde(default = "default_wildcard")]
    pub tenants: Vec<String>,
    /// Optional namespace scope. When set, this signer can only sign
    /// actions for the listed namespaces. A wildcard `["*"]` (the
    /// default) allows all namespaces.
    #[serde(default = "default_wildcard")]
    pub namespaces: Vec<String>,
}

fn default_kid() -> String {
    acteon_crypto::signing::DEFAULT_KID.to_owned()
}

fn default_wildcard() -> Vec<String> {
    vec!["*".into()]
}
