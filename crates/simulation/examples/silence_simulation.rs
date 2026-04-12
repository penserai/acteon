//! Silence lifecycle simulation scenarios.
//!
//! Demonstrates the end-to-end create → intercept → expire cycle for
//! silences, plus multi-matcher AND semantics, regex matching,
//! hierarchical tenant coverage, and the interaction between silences
//! and rule verdicts.
//!
//! Run with: `cargo run -p acteon-simulation --example silence_simulation`

use acteon_core::{Action, ActionMetadata, ActionOutcome, MatchOp, Silence, SilenceMatcher};
use acteon_simulation::prelude::*;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use tracing::info;

/// Build an [`ActionMetadata`] from a slice of key-value pairs.
fn labels(pairs: &[(&str, &str)]) -> ActionMetadata {
    ActionMetadata {
        labels: pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<HashMap<_, _>>(),
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║               SILENCE SIMULATION DEMO                       ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Basic silence — matching dispatch is muted
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: BASIC SILENCE — severity=warning dispatches muted");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .build(),
    )
    .await?;
    info!("✓ Started simulation cluster with 1 node\n");

    // Create a silence that mutes severity=warning for 1 hour.
    let now = Utc::now();
    let silence = Silence {
        id: "silence-1".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        matchers: vec![SilenceMatcher::new("severity", "warning", MatchOp::Equal)?],
        starts_at: now,
        ends_at: now + Duration::hours(1),
        created_by: "simulation".into(),
        comment: "Mute warnings during canary deploy".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_silence(&silence).await?;
    gw.upsert_silence_cache(silence.clone())?;
    info!("✓ Created silence: severity=warning for prod/acme (1h)");
    info!("  ID: {}", silence.id);
    info!("  Comment: {}\n", silence.comment);

    // Dispatch a warning → silenced.
    let warning_action = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "disk usage above 80%"}),
    )
    .with_metadata(labels(&[("severity", "warning")]));
    info!("→ Dispatching severity=warning...");
    let outcome = harness.dispatch(&warning_action).await?;
    info!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Silenced { .. }));
    info!("  ✓ Correctly silenced\n");

    // Dispatch a critical → NOT silenced, executes normally.
    let critical_action = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "database connection pool exhausted"}),
    )
    .with_metadata(labels(&[("severity", "critical")]));
    info!("→ Dispatching severity=critical (should NOT be silenced)...");
    let outcome = harness.dispatch(&critical_action).await?;
    info!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed { .. }));
    info!("  ✓ Correctly executed (severity does not match silence)\n");

    let provider = harness.provider("log").unwrap();
    info!(
        "  Provider call count: {} (only the critical alert)",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 1);
    harness.teardown().await?;
    info!("✓ Demo 1 complete\n");

    // =========================================================================
    // DEMO 2: Multi-matcher AND semantics
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: MULTI-MATCHER AND — both must match to silence");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .build(),
    )
    .await?;

    let now = Utc::now();
    let silence = Silence {
        id: "silence-and".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        matchers: vec![
            SilenceMatcher::new("severity", "warning", MatchOp::Equal)?,
            SilenceMatcher::new("service", "cdn-edge", MatchOp::Equal)?,
        ],
        starts_at: now,
        ends_at: now + Duration::hours(2),
        created_by: "simulation".into(),
        comment: "Mute CDN warnings during maintenance".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_silence(&silence).await?;
    gw.upsert_silence_cache(silence)?;
    info!("✓ Created silence: severity=warning AND service=cdn-edge\n");

    // Both match → silenced.
    let cdn_warning = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "CDN hit rate low"}),
    )
    .with_metadata(labels(&[("severity", "warning"), ("service", "cdn-edge")]));
    info!("→ severity=warning + service=cdn-edge...");
    let outcome = harness.dispatch(&cdn_warning).await?;
    assert!(matches!(outcome, ActionOutcome::Silenced { .. }));
    info!("  ✓ Silenced (both matchers match)\n");

    // Only one matches → NOT silenced.
    let db_warning = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "DB latency spike"}),
    )
    .with_metadata(labels(&[
        ("severity", "warning"),
        ("service", "postgres-primary"),
    ]));
    info!("→ severity=warning + service=postgres-primary...");
    let outcome = harness.dispatch(&db_warning).await?;
    assert!(matches!(outcome, ActionOutcome::Executed { .. }));
    info!("  ✓ Executed (service does not match — AND requires both)\n");

    harness.teardown().await?;
    info!("✓ Demo 2 complete\n");

    // =========================================================================
    // DEMO 3: Regex matcher
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: REGEX MATCHER — pattern-based label matching");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .build(),
    )
    .await?;

    let now = Utc::now();
    let silence = Silence {
        id: "silence-regex".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        matchers: vec![SilenceMatcher::new("service", "cdn-.*", MatchOp::Regex)?],
        starts_at: now,
        ends_at: now + Duration::hours(1),
        created_by: "simulation".into(),
        comment: "Regex: silence all cdn-* services".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_silence(&silence).await?;
    gw.upsert_silence_cache(silence)?;
    info!("✓ Created silence: service=~\"cdn-.*\"\n");

    for (service, expect_silenced) in [
        ("cdn-edge", true),
        ("cdn-origin", true),
        ("checkout-api", false),
    ] {
        let action = Action::new(
            "prod",
            "acme",
            "log",
            "alert",
            serde_json::json!({"message": format!("alert from {service}")}),
        )
        .with_metadata(labels(&[("service", service)]));
        let outcome = harness.dispatch(&action).await?;
        let silenced = matches!(outcome, ActionOutcome::Silenced { .. });
        info!(
            "  service={service:<16} → {}",
            if silenced { "SILENCED" } else { "EXECUTED" }
        );
        assert_eq!(silenced, expect_silenced);
    }
    info!("  ✓ Regex correctly matched cdn-edge and cdn-origin but not checkout-api\n");

    harness.teardown().await?;
    info!("✓ Demo 3 complete\n");

    // =========================================================================
    // DEMO 4: Hierarchical tenant coverage
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: HIERARCHICAL TENANT — parent silence covers children");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .build(),
    )
    .await?;

    let now = Utc::now();
    let silence = Silence {
        id: "silence-parent".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        matchers: vec![SilenceMatcher::new("severity", "info", MatchOp::Equal)?],
        starts_at: now,
        ends_at: now + Duration::hours(1),
        created_by: "simulation".into(),
        comment: "Parent tenant silence covers child tenants".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_silence(&silence).await?;
    gw.upsert_silence_cache(silence)?;
    info!("✓ Created silence on tenant=acme for severity=info\n");

    // Dispatch to child tenant acme.us-east → covered by parent silence.
    let child = Action::new(
        "prod",
        "acme.us-east",
        "log",
        "alert",
        serde_json::json!({"message": "info from child tenant"}),
    )
    .with_metadata(labels(&[("severity", "info")]));
    info!("→ tenant=acme.us-east, severity=info...");
    let outcome = harness.dispatch(&child).await?;
    assert!(matches!(outcome, ActionOutcome::Silenced { .. }));
    info!("  ✓ Silenced (parent tenant=acme covers acme.us-east)\n");

    // Dispatch to unrelated tenant → NOT covered.
    let other = Action::new(
        "prod",
        "other-org",
        "log",
        "alert",
        serde_json::json!({"message": "info from different org"}),
    )
    .with_metadata(labels(&[("severity", "info")]));
    info!("→ tenant=other-org, severity=info...");
    let outcome = harness.dispatch(&other).await?;
    assert!(matches!(outcome, ActionOutcome::Executed { .. }));
    info!("  ✓ Executed (different tenant)\n");

    harness.teardown().await?;
    info!("✓ Demo 4 complete\n");

    // =========================================================================
    // DEMO 5: Expire and resume
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 5: EXPIRE AND RESUME — silence removed, dispatches resume");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .build(),
    )
    .await?;

    let now = Utc::now();
    let silence = Silence {
        id: "silence-expire".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        matchers: vec![SilenceMatcher::new("severity", "warning", MatchOp::Equal)?],
        starts_at: now,
        ends_at: now + Duration::hours(1),
        created_by: "simulation".into(),
        comment: "Will be expired mid-demo".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_silence(&silence).await?;
    gw.upsert_silence_cache(silence)?;
    info!("✓ Created silence: severity=warning");

    let action = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "warning before expire"}),
    )
    .with_metadata(labels(&[("severity", "warning")]));

    // Before expire → silenced.
    info!("→ Dispatch before expire...");
    let outcome = harness.dispatch(&action).await?;
    assert!(matches!(outcome, ActionOutcome::Silenced { .. }));
    info!("  ✓ Silenced");

    // Expire the silence by removing from cache.
    gw.remove_silence_cache("prod", "acme", "silence-expire");
    info!("→ Silence expired (removed from cache)");

    // After expire → executes.
    let action2 = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "warning after expire"}),
    )
    .with_metadata(labels(&[("severity", "warning")]));
    info!("→ Dispatch after expire...");
    let outcome = harness.dispatch(&action2).await?;
    assert!(matches!(outcome, ActionOutcome::Executed { .. }));
    info!("  ✓ Executed (silence no longer active)");

    let provider = harness.provider("log").unwrap();
    info!(
        "\n  Provider call count: {} (only the post-expire dispatch)",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 1);

    harness.teardown().await?;
    info!("\n✓ All demos complete.");
    Ok(())
}
