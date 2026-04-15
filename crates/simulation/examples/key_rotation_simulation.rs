//! End-to-end key rotation lifecycle demo for action signing.
//!
//! Walks through the rotation pattern documented in
//! `docs/book/features/action-signing.md`:
//!
//!   1. Provision the initial key (`k1`).
//!   2. Sign + verify an action under `k1`.
//!   3. Stage a new key (`k2`) alongside `k1` — both active.
//!   4. Verify a `k1`-signed action AND a `k2`-signed action both pass.
//!   5. Migrate clients to send `kid: "k2"` on dispatch.
//!   6. Retire `k1` — old signatures now rejected, new signatures pass.
//!
//! This example doesn't spin up a full server cluster; it exercises
//! the [`Keyring`](acteon_crypto::signing::Keyring) directly so the
//! rotation API is easy to follow without harness glue. For a full
//! signed dispatch through the gateway, see
//! `crates/client/examples` or the action signing docs.
//!
//! Run with:
//!
//! ```text
//! cargo run -p acteon-simulation --example key_rotation_simulation
//! ```

use acteon_core::Action;
use acteon_crypto::signing::{Keyring, generate_keypair_with_kid};
use tracing::info;

#[allow(clippy::too_many_lines)]
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║              KEY ROTATION SIMULATION                         ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // Build a representative action that we'll re-sign at each step.
    let action = Action::new(
        "prod",
        "acme",
        "email",
        "send_alert",
        serde_json::json!({"to": "oncall@example.com", "subject": "DB unhealthy"}),
    );
    let canonical = action.canonical_bytes();

    // =========================================================================
    // PHASE 1: Initial deployment with one key (k1)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 1: INITIAL DEPLOYMENT — single key 'ci-bot/k1'");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let (sk_k1, vk_k1) = generate_keypair_with_kid("ci-bot", "k1");
    let mut keyring = Keyring::new();
    keyring.insert(vk_k1);
    info!(
        "✓ Keyring contains {} key(s); ci-bot/k1 active",
        keyring.len()
    );

    // Sign + verify with explicit kid.
    let sig_k1 = sk_k1.sign(&canonical);
    info!("→ Client signs the action with k1, sends kid=\"k1\" on dispatch");
    keyring
        .verify_with_kid("ci-bot", "k1", &sig_k1, &canonical)
        .expect("k1 signature must verify under k1");
    info!("  ✓ Server verifies via verify_with_kid(\"ci-bot\", \"k1\", ..)\n");

    // Legacy clients that don't know about kid still work via the
    // try-all-keys-for-signer fallback.
    info!("→ Legacy client signs without kid; server falls back to try-all");
    keyring
        .verify("ci-bot", &sig_k1, &canonical)
        .expect("legacy verify finds the matching kid");
    info!("  ✓ Server verifies via legacy verify(\"ci-bot\", ..)\n");

    // =========================================================================
    // PHASE 2: Stage a new key alongside k1
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 2: STAGE ROTATION — add 'ci-bot/k2' alongside 'ci-bot/k1'");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let (sk_k2, vk_k2) = generate_keypair_with_kid("ci-bot", "k2");
    keyring.insert(vk_k2);
    info!(
        "✓ Keyring now contains {} key(s) for ci-bot",
        keyring
            .iter_keys()
            .filter(|k| k.signer_id() == "ci-bot")
            .count()
    );
    info!("  /.well-known/acteon-signing-keys would now publish both kids\n");

    // Both keys verify their own signatures.
    let sig_k2 = sk_k2.sign(&canonical);
    keyring
        .verify_with_kid("ci-bot", "k1", &sig_k1, &canonical)
        .expect("k1 signature still verifies");
    keyring
        .verify_with_kid("ci-bot", "k2", &sig_k2, &canonical)
        .expect("k2 signature verifies");
    info!("  ✓ k1 signature still valid (in-flight legacy traffic protected)");
    info!("  ✓ k2 signature also valid (new clients can start using k2)\n");

    // Cross-key rejection: a k1 signature must NOT verify under k2.
    let cross_check = keyring.verify_with_kid("ci-bot", "k2", &sig_k1, &canonical);
    assert!(
        cross_check.is_err(),
        "k1 signature should NOT verify under k2"
    );
    info!("  ✓ k1 signature correctly rejected when verified under k2");
    info!("    (verify_with_kid is strict — never silently tries other kids)\n");

    // =========================================================================
    // PHASE 3: Migration — clients flip to k2, k1 still tolerated
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 3: MIGRATE — clients send kid=\"k2\", k1 stays active");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("→ A migrated client signs with k2 and sends kid=\"k2\"");
    keyring
        .verify_with_kid("ci-bot", "k2", &sig_k2, &canonical)
        .expect("migrated client passes verification");
    info!("  ✓ Verified against k2");

    info!("→ A laggard client still uses k1 — same audit trail, no rejection");
    keyring
        .verify_with_kid("ci-bot", "k1", &sig_k1, &canonical)
        .expect("laggard client still passes");
    info!(
        "  ✓ Verified against k1 — both kids active until the longest in-flight\n    \
         signed action ages out\n"
    );

    // =========================================================================
    // PHASE 4: Retire the old key
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 4: RETIRE — remove 'ci-bot/k1' from the keyring");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let removed = keyring.remove("ci-bot", "k1");
    assert!(removed, "k1 should be present before removal");
    info!("✓ k1 removed; keyring now has only ci-bot/k2");
    assert!(!keyring.contains_kid("ci-bot", "k1"));
    assert!(keyring.contains_kid("ci-bot", "k2"));

    info!("→ A laggard client that didn't migrate sends a k1 signature");
    let post_retire = keyring.verify_with_kid("ci-bot", "k1", &sig_k1, &canonical);
    assert!(
        post_retire.is_err(),
        "k1 signature must be rejected after retirement"
    );
    info!("  ✓ Server rejects: UnknownSigner(\"ci-bot/k1\")");
    info!("    (operator monitoring should alert when this fires post-rotation)");

    info!("→ A migrated client sends a k2 signature");
    keyring
        .verify_with_kid("ci-bot", "k2", &sig_k2, &canonical)
        .expect("k2 still active");
    info!("  ✓ Verified against k2 — rotation complete\n");

    // =========================================================================
    // Wrap-up
    // =========================================================================
    info!("════════════════════════════════════════════════════════════════");
    info!("  KEY ROTATION COMPLETE");
    info!("════════════════════════════════════════════════════════════════");
    info!("");
    info!("Summary of the rotation lifecycle:");
    info!("  Phase 1 → 1 key (k1), legacy + kid-aware clients both verify");
    info!("  Phase 2 → 2 keys (k1+k2), staged for rotation, both active");
    info!("  Phase 3 → migration window, clients flip to k2 at their pace");
    info!("  Phase 4 → 1 key (k2), k1 retired, old signatures rejected");
    info!("");
    info!("Operators stage rotations by editing [signing.keyring] in TOML");
    info!("and restarting (or reloading) the server. The verifier set is");
    info!("publicly discoverable at /.well-known/acteon-signing-keys.");
}
