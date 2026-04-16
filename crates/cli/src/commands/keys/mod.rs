//! `acteon keys` — local Ed25519 key management for action signing.
//!
//! These commands run entirely client-side; they do not talk to a
//! running gateway. They exist to remove the bootstrap friction of
//! enabling the `[signing]` feature: instead of piping `openssl
//! genpkey` and base64 by hand, an operator runs
//!
//!     acteon keys generate ci-bot
//!
//! and gets a fresh keypair plus a ready-to-paste TOML keyring entry.
//! See `docs/book/features/action-signing.md` for the full rotation
//! workflow.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use clap::{Args, Subcommand, ValueEnum};
use serde::Deserialize;
use zeroize::Zeroizing;

use acteon_crypto::signing::{ActionSigningKey, DEFAULT_KID, generate_keypair_with_kid};

/// Output encoding for raw key material.
///
/// Operators in CI/CD generally prefer `hex` (round-trips cleanly
/// through environment variables and shell scripts); the `base64`
/// option exists for parity with the wire format used inside
/// `signing.keyring[].public_key`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum KeyEncoding {
    Hex,
    Base64,
}

impl KeyEncoding {
    fn encode(self, bytes: &[u8]) -> String {
        match self {
            Self::Hex => hex::encode(bytes),
            Self::Base64 => B64.encode(bytes),
        }
    }
}

/// `acteon keys` umbrella.
#[derive(Args, Debug)]
pub struct KeysArgs {
    #[command(subcommand)]
    pub command: KeysCommand,
}

#[derive(Subcommand, Debug)]
pub enum KeysCommand {
    /// Generate a fresh Ed25519 keypair for action signing.
    ///
    /// Prints the secret key, public key, and a ready-to-paste TOML
    /// `[[signing.keyring]]` entry. The secret is emitted to stdout
    /// once and never persisted by this command — capture it into a
    /// secret manager immediately.
    Generate(GenerateArgs),
    /// List the active signing keys configured in a server config file.
    ///
    /// Reads `[[signing.keyring]]` entries from a TOML config and
    /// prints them in a compact table. Useful before a rotation to
    /// see which `(signer_id, kid)` pairs are already registered.
    List(ListArgs),
    /// Generate a fresh keypair under the next free `kid` for an
    /// existing signer.
    ///
    /// Reads the existing config to find the highest `kid` already
    /// registered for `--signer-id`, generates a new keypair under
    /// the next sequential `kid`, and prints the new TOML keyring
    /// entry to append. Read-only on the config file; the operator
    /// pastes the result themselves.
    Rotate(RotateArgs),
}

#[derive(Args, Debug)]
pub struct GenerateArgs {
    /// Logical signer identity stamped on every signature this key
    /// produces. This becomes the `signer_id` field on signed
    /// actions and the `[[signing.keyring]].signer_id` in the server
    /// config.
    #[arg(value_name = "SIGNER_ID")]
    pub signer_id: String,

    /// Optional key identifier within the signer. Defaults to `k0`
    /// (the same default the verifier uses for legacy single-key
    /// entries).
    #[arg(long, default_value = DEFAULT_KID)]
    pub kid: String,

    /// Encoding for the secret/public key bytes in the printed
    /// output. The TOML keyring entry always uses base64 because
    /// that's what the server parser accepts in either form.
    #[arg(long, value_enum, default_value_t = KeyEncoding::Hex)]
    pub encoding: KeyEncoding,

    /// Write the secret key to this file instead of stderr.
    ///
    /// When set, the secret is written with mode 0600 on Unix (no
    /// group/world read). Nothing about the secret touches stdout
    /// or stderr in this mode, so it's safe to run inside a CI job
    /// that log-captures both streams — as long as the configured
    /// path lands in a filesystem the secret scraper can't reach
    /// (e.g. a tmpfs mount or a path that the CI runner evicts).
    #[arg(long, value_name = "PATH")]
    pub secret_out: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Path to an `acteon.toml` (or any file containing a `[signing]`
    /// section) to read the keyring from.
    #[arg(value_name = "CONFIG")]
    pub config: PathBuf,
}

#[derive(Args, Debug)]
pub struct RotateArgs {
    /// Path to the existing server config to scan for the highest
    /// `kid` already registered under `--signer-id`. Read-only — the
    /// command never modifies the file.
    #[arg(value_name = "CONFIG")]
    pub config: PathBuf,

    /// Signer to rotate.
    #[arg(long)]
    pub signer_id: String,

    /// Encoding for the printed secret/public key bytes.
    #[arg(long, value_enum, default_value_t = KeyEncoding::Hex)]
    pub encoding: KeyEncoding,

    /// Write the secret key to this file instead of stderr. Same
    /// semantics as `keys generate --secret-out`.
    #[arg(long, value_name = "PATH")]
    pub secret_out: Option<PathBuf>,
}

pub fn run(args: &KeysArgs) -> anyhow::Result<()> {
    match &args.command {
        KeysCommand::Generate(a) => run_generate(a),
        KeysCommand::List(a) => run_list(a),
        KeysCommand::Rotate(a) => run_rotate(a),
    }
}

fn run_generate(args: &GenerateArgs) -> anyhow::Result<()> {
    let (sk, vk) = generate_keypair_with_kid(&args.signer_id, &args.kid);
    emit_keypair(
        &args.signer_id,
        &args.kid,
        &sk,
        &vk,
        args.encoding,
        args.secret_out.as_deref(),
    )
}

fn run_list(args: &ListArgs) -> anyhow::Result<()> {
    let entries = read_keyring(&args.config)?;
    if entries.is_empty() {
        println!(
            "(no [[signing.keyring]] entries in {})",
            args.config.display()
        );
        return Ok(());
    }

    // Compute per-column widths from actual data so long signer_ids
    // (e.g. `prod-us-east-1-deploy-service`) or comma-joined
    // tenant/namespace lists don't overflow fixed-width padding and
    // break the table layout. Headers contribute to the minimum so
    // short data never collapses the columns below the label width.
    let rows: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e.signer_id.as_str(),
                e.kid.as_str(),
                e.tenants.join(","),
                e.namespaces.join(","),
            )
        })
        .collect();

    let w_signer = rows
        .iter()
        .map(|r| r.0.len())
        .max()
        .unwrap_or(0)
        .max("SIGNER_ID".len());
    let w_kid = rows
        .iter()
        .map(|r| r.1.len())
        .max()
        .unwrap_or(0)
        .max("KID".len());
    let w_tenants = rows
        .iter()
        .map(|r| r.2.len())
        .max()
        .unwrap_or(0)
        .max("TENANTS".len());

    println!(
        "{:<w_signer$}  {:<w_kid$}  {:<w_tenants$}  NAMESPACES",
        "SIGNER_ID", "KID", "TENANTS"
    );
    for (signer, kid, tenants, namespaces) in &rows {
        println!("{signer:<w_signer$}  {kid:<w_kid$}  {tenants:<w_tenants$}  {namespaces}");
    }
    Ok(())
}

fn run_rotate(args: &RotateArgs) -> anyhow::Result<()> {
    let entries = read_keyring(&args.config)?;
    let next_kid = next_kid_for(&entries, &args.signer_id);

    let (sk, vk) = generate_keypair_with_kid(&args.signer_id, &next_kid);

    eprintln!(
        "Rotating signer '{}': existing kids = [{}], next = {}",
        args.signer_id,
        entries
            .iter()
            .filter(|e| e.signer_id == args.signer_id)
            .map(|e| e.kid.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        next_kid,
    );
    eprintln!();
    emit_keypair(
        &args.signer_id,
        &next_kid,
        &sk,
        &vk,
        args.encoding,
        args.secret_out.as_deref(),
    )?;
    eprintln!();
    eprintln!(
        "After capturing the SECRET, append the keyring entry above to {}, \n\
         restart the server, then migrate signers to send 'kid: \"{}\"'.",
        args.config.display(),
        next_kid,
    );
    Ok(())
}

/// Emit a freshly generated keypair.
///
/// Stream split:
/// - **stdout** receives only the ready-to-paste
///   `[[signing.keyring]]` block (public material only). Safe to
///   redirect into `>> config.toml`.
/// - **stderr** receives the human-readable header, the secret key
///   (unless `--secret-out` was set), the public key, and the
///   "append this to your config" hint.
/// - When `secret_out` is `Some`, the raw secret bytes are written
///   to that file (mode 0600 on Unix, plain write on Windows)
///   instead of to stderr. This is the recommended path for any
///   automated / CI context where both stdout and stderr may be
///   captured into logs.
///
/// The secret bytes live in a `Zeroizing<[u8; 32]>` wrapper for the
/// duration of this function so the stack copy is wiped as soon as
/// the wrapper drops — the underlying `ActionSigningKey` also
/// zeroizes on drop, but an unwrapped `[u8; 32]` returned from
/// `to_bytes()` would otherwise linger until the stack frame is
/// overwritten by an unrelated call.
fn emit_keypair(
    signer_id: &str,
    kid: &str,
    sk: &ActionSigningKey,
    vk: &acteon_crypto::signing::ActionVerifyingKey,
    encoding: KeyEncoding,
    secret_out: Option<&Path>,
) -> anyhow::Result<()> {
    let secret_bytes: Zeroizing<[u8; 32]> = Zeroizing::new(sk.to_bytes());
    let public_bytes = vk.public_key_bytes();
    let secret_encoded = Zeroizing::new(encoding.encode(secret_bytes.as_ref()));

    // --- Human-readable block on stderr ---
    eprintln!("# Acteon signing keypair");
    eprintln!("# signer_id = {signer_id}");
    eprintln!("# kid       = {kid}");
    eprintln!();

    if let Some(path) = secret_out {
        write_secret_file(path, secret_encoded.as_str())?;
        eprintln!(
            "SECRET written to {} (mode 0600 on Unix). Move it to your secret manager.",
            path.display()
        );
    } else {
        eprintln!("SECRET (capture into a secret manager NOW — printed once):");
        eprintln!("  {}", secret_encoded.as_str());
    }
    eprintln!();
    eprintln!("PUBLIC:");
    eprintln!("  {}", encoding.encode(&public_bytes));
    eprintln!();
    eprintln!("# Append the block on stdout to your server config:");

    // --- TOML stub on stdout — safe to pipe into a file ---
    println!("[[signing.keyring]]");
    println!("signer_id = \"{signer_id}\"");
    println!("kid = \"{kid}\"");
    println!("public_key = \"{}\"", B64.encode(public_bytes));
    println!("# tenants = [\"*\"]      # uncomment to scope");
    println!("# namespaces = [\"*\"]   # uncomment to scope");

    Ok(())
}

/// Write the secret key to `path` with tight permissions.
///
/// On Unix, the file is created with mode 0600 (owner read/write
/// only) so a shared CI runner or container with multiple
/// unprivileged processes can't read it. On Windows we fall back to
/// a plain write — the ACL story differs enough that operators
/// there should be setting permissions via the host's own tooling.
fn write_secret_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to open {} for writing", path.display()))?;
        f.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write secret to {}", path.display()))?;
        // Trailing newline is convenient when the file is `cat`ed,
        // and harmless when parsed back by `parse_signing_key`
        // (which `.trim()`s its input).
        f.write_all(b"\n")
            .with_context(|| format!("failed to write newline to {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, format!("{contents}\n"))
            .with_context(|| format!("failed to write secret to {}", path.display()))?;
    }
    Ok(())
}

// --- TOML parsing ----------------------------------------------------------
//
// We define a minimal local schema rather than depending on
// `acteon-server` so the CLI doesn't need to compile every server
// crate just to read a [signing] section. The shape mirrors
// `acteon_server::config::signing::SigningConfig` exactly, and the
// `read_keyring` helper is unit-tested below to catch drift.

#[derive(Debug, Deserialize)]
struct SigningSection {
    #[serde(default)]
    keyring: Vec<KeyringEntry>,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    signing: Option<SigningSection>,
}

#[derive(Debug, Deserialize)]
struct KeyringEntry {
    signer_id: String,
    #[serde(default = "default_kid")]
    kid: String,
    #[serde(default)]
    #[allow(dead_code)] // surfaced via the printed table
    public_key: String,
    #[serde(default = "default_wildcard")]
    tenants: Vec<String>,
    #[serde(default = "default_wildcard")]
    namespaces: Vec<String>,
}

fn default_kid() -> String {
    DEFAULT_KID.to_owned()
}

fn default_wildcard() -> Vec<String> {
    vec!["*".to_owned()]
}

fn read_keyring(path: &std::path::Path) -> anyhow::Result<Vec<KeyringEntry>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    let parsed: ConfigFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse TOML config: {}", path.display()))?;
    Ok(parsed.signing.map(|s| s.keyring).unwrap_or_default())
}

/// Pick the next sequential `kid` for `signer_id` given the current
/// keyring entries. Strategy:
///
/// 1. Collect every existing kid for the signer.
/// 2. If any kid matches the pattern `k<digit>`, find the highest
///    integer suffix and return `k<n+1>`.
/// 3. Otherwise (no entries, or only non-numeric kids), return `k1`.
///
/// This deliberately doesn't try to invent clever schemes for
/// non-numeric kids — operators who use `prod-2026-04` style kids
/// already have their own naming convention and shouldn't have the
/// CLI second-guess them. They can pass `--kid` to `generate`
/// directly.
fn next_kid_for(entries: &[KeyringEntry], signer_id: &str) -> String {
    let highest = entries
        .iter()
        .filter(|e| e.signer_id == signer_id)
        .filter_map(|e| e.kid.strip_prefix('k').and_then(|s| s.parse::<u32>().ok()))
        .max();
    match highest {
        Some(n) => format!("k{}", n + 1),
        None => "k1".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_parse_round_trip() {
        // Generate a key, format it, parse it back, and verify the
        // bytes match. Closes the loop on the to_bytes() / encode /
        // parse_signing_key path that the CLI exposes to operators.
        let (sk, _vk) = generate_keypair_with_kid("test-signer", "k0");
        let bytes = sk.to_bytes();
        let hex_encoded = KeyEncoding::Hex.encode(&bytes);
        let b64_encoded = KeyEncoding::Base64.encode(&bytes);

        let parsed_hex =
            acteon_crypto::signing::parse_signing_key(&hex_encoded, "test-signer").unwrap();
        let parsed_b64 =
            acteon_crypto::signing::parse_signing_key(&b64_encoded, "test-signer").unwrap();

        assert_eq!(parsed_hex.to_bytes(), bytes);
        assert_eq!(parsed_b64.to_bytes(), bytes);
    }

    #[test]
    fn read_keyring_parses_minimal_signing_section() {
        let toml_src = r#"
[signing]
enabled = true

[[signing.keyring]]
signer_id = "ci-bot"
kid = "k1"
public_key = "AAAA"
tenants = ["acme"]
namespaces = ["prod", "staging"]

[[signing.keyring]]
signer_id = "deploy-svc"
public_key = "BBBB"
"#;
        let parsed: ConfigFile = toml::from_str(toml_src).unwrap();
        let entries = parsed.signing.unwrap().keyring;

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].signer_id, "ci-bot");
        assert_eq!(entries[0].kid, "k1");
        assert_eq!(entries[0].tenants, vec!["acme"]);
        // Defaults applied to the second entry (no kid, no scopes)
        assert_eq!(entries[1].signer_id, "deploy-svc");
        assert_eq!(entries[1].kid, "k0");
        assert_eq!(entries[1].tenants, vec!["*"]);
        assert_eq!(entries[1].namespaces, vec!["*"]);
    }

    #[test]
    fn read_keyring_handles_missing_signing_section() {
        // A config with no [signing] block at all should return an
        // empty keyring rather than erroring — `keys list` is a
        // legitimate query against a not-yet-configured server.
        let parsed: ConfigFile = toml::from_str("[server]\nport = 8080").unwrap();
        assert!(parsed.signing.is_none());
    }

    fn entry(signer: &str, kid: &str) -> KeyringEntry {
        KeyringEntry {
            signer_id: signer.to_owned(),
            kid: kid.to_owned(),
            public_key: String::new(),
            tenants: vec!["*".to_owned()],
            namespaces: vec!["*".to_owned()],
        }
    }

    #[test]
    fn next_kid_for_empty_keyring_starts_at_k1() {
        // Fresh signer with no prior keys — start at k1, not k0,
        // because k0 is the legacy default and a new signer should
        // begin its rotation history with an explicit identifier.
        let entries: Vec<KeyringEntry> = vec![];
        assert_eq!(next_kid_for(&entries, "ci-bot"), "k1");
    }

    #[test]
    fn next_kid_for_picks_highest_plus_one() {
        let entries = vec![
            entry("ci-bot", "k1"),
            entry("ci-bot", "k2"),
            entry("ci-bot", "k5"), // gap is fine — we always pick max+1
            entry("deploy-svc", "k1"),
        ];
        assert_eq!(next_kid_for(&entries, "ci-bot"), "k6");
        assert_eq!(next_kid_for(&entries, "deploy-svc"), "k2");
    }

    #[test]
    fn next_kid_for_skips_non_numeric_kids() {
        // Operator using a custom naming scheme — we ignore those
        // entirely and start fresh at k1. They can pass --kid to
        // generate if they want to keep their convention.
        let entries = vec![
            entry("ci-bot", "prod-2026-04"),
            entry("ci-bot", "prod-2026-05"),
        ];
        assert_eq!(next_kid_for(&entries, "ci-bot"), "k1");
    }

    #[test]
    fn next_kid_for_isolates_per_signer() {
        // Two signers with overlapping kid namespaces shouldn't
        // affect each other.
        let entries = vec![
            entry("ci-bot", "k1"),
            entry("ci-bot", "k2"),
            entry("deploy-svc", "k7"),
        ];
        assert_eq!(next_kid_for(&entries, "ci-bot"), "k3");
        assert_eq!(next_kid_for(&entries, "deploy-svc"), "k8");
        // A signer that doesn't exist yet starts at k1 too.
        assert_eq!(next_kid_for(&entries, "phantom"), "k1");
    }
}
