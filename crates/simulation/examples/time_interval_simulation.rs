//! Time interval lifecycle simulation.
//!
//! Demonstrates how rules reference [`TimeInterval`]s through their
//! `mute_time_intervals` / `active_time_intervals` fields, and how the
//! gateway short-circuits matching dispatches to
//! [`ActionOutcome::Muted`] when the schedule says the rule should not
//! fire right now.
//!
//! Run with:
//!
//! ```text
//! cargo run -p acteon-simulation --example time_interval_simulation
//! ```

use acteon_core::time_interval::{TimeOfDayRange, TimeRange, WeekdayRange};
use acteon_core::{Action, ActionOutcome, TimeInterval};
use acteon_simulation::prelude::*;
use chrono::Utc;
use tracing::info;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║              TIME INTERVAL SIMULATION DEMO                   ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Always-matching mute interval blocks dispatches
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: MUTE WINDOW — rule muted by always-matching interval");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Rule that allows alerts but is gated by `always-on` mute interval.
    let rule_yaml = r#"
rules:
  - name: muted-allow
    priority: 1
    condition:
      field: action.action_type
      eq: alert
    action:
      type: allow
    mute_time_intervals:
      - always-on
"#;

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_rule_yaml(rule_yaml)
            .build(),
    )
    .await?;
    info!("✓ Started cluster with one rule referencing 'always-on' interval\n");

    let now = Utc::now();
    let always_on = TimeInterval {
        name: "always-on".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        // No predicates → an empty TimeRange matches every instant.
        time_ranges: vec![TimeRange::default()],
        location: None,
        description: Some("Always matches — used for the mute demo".into()),
        created_by: "simulation".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_time_interval(&always_on).await?;
    gw.upsert_time_interval_cache(always_on.clone())?;
    info!("✓ Registered time interval 'always-on' (empty predicate = match always)\n");

    let action = Action::new(
        "prod",
        "acme",
        "log",
        "alert",
        serde_json::json!({"message": "disk usage above 80%"}),
    );
    info!("→ Dispatching alert (rule matches, then time interval mutes)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Muted { .. }));
    info!("  ✓ Correctly muted by interval\n");

    let provider = harness.provider("log").unwrap();
    assert_eq!(
        provider.call_count(),
        0,
        "muted dispatches must not call providers"
    );
    info!("  Provider call count: 0 (provider untouched)\n");
    harness.teardown().await?;
    info!("✓ Demo 1 complete\n");

    // =========================================================================
    // DEMO 2: Interval that does NOT match — dispatch executes
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: NON-MATCHING MUTE — interval idle, dispatch proceeds");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_rule_yaml(rule_yaml)
            .build(),
    )
    .await?;

    // Build an interval whose only TimeRange targets a year far in the
    // past — so it never matches "now".
    let never_match = TimeInterval {
        name: "always-on".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        time_ranges: vec![TimeRange {
            years: vec![acteon_core::time_interval::YearRange {
                start: 1970,
                end: 1970,
            }],
            ..Default::default()
        }],
        location: None,
        description: Some("Never matches in the present".into()),
        created_by: "simulation".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_time_interval(&never_match).await?;
    gw.upsert_time_interval_cache(never_match)?;
    info!("✓ Registered 'always-on' interval scoped to year 1970\n");

    info!("→ Dispatching alert (rule matches, mute interval idle)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed { .. }));
    info!("  ✓ Dispatch executed normally\n");

    let provider = harness.provider("log").unwrap();
    assert_eq!(provider.call_count(), 1);
    info!("  Provider call count: 1\n");
    harness.teardown().await?;
    info!("✓ Demo 2 complete\n");

    // =========================================================================
    // DEMO 3: Active-time-intervals — outside window mutes the rule
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: ACTIVE WINDOW — rule muted because we're outside it");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let active_rule = r#"
rules:
  - name: active-only
    priority: 1
    condition:
      field: action.action_type
      eq: alert
    action:
      type: allow
    active_time_intervals:
      - business-hours
"#;

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_rule_yaml(active_rule)
            .build(),
    )
    .await?;

    // Business hours window 09:00-17:00, weekdays only — but in a
    // year that won't match, so it's guaranteed inactive regardless of
    // when the simulation runs. (Demo 4 below covers the "matching"
    // case using an always-on `TimeRange::default()`.)
    let business_hours = TimeInterval {
        name: "business-hours".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        time_ranges: vec![TimeRange {
            times: vec![TimeOfDayRange::from_hm(9, 0, 17, 0)?],
            weekdays: vec![WeekdayRange { start: 1, end: 5 }],
            years: vec![acteon_core::time_interval::YearRange {
                start: 1970,
                end: 1970,
            }],
            ..Default::default()
        }],
        location: Some("UTC".into()),
        description: None,
        created_by: "simulation".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_time_interval(&business_hours).await?;
    gw.upsert_time_interval_cache(business_hours)?;
    info!("✓ Registered 'business-hours' interval scoped to 1970 (always inactive)\n");

    info!("→ Dispatching alert (active window not matched → muted)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Muted { .. }));
    info!("  ✓ Correctly muted for being outside the active window\n");

    harness.teardown().await?;
    info!("✓ Demo 3 complete\n");

    // =========================================================================
    // DEMO 4: Active-time-intervals — inside window allows dispatch
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: ACTIVE WINDOW — rule fires while interval matches");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_rule_yaml(active_rule)
            .build(),
    )
    .await?;

    let always_active = TimeInterval {
        name: "business-hours".into(),
        namespace: "prod".into(),
        tenant: "acme".into(),
        // Empty range = match every instant, used here to simulate an
        // interval that's currently in its active window.
        time_ranges: vec![TimeRange::default()],
        location: None,
        description: None,
        created_by: "simulation".into(),
        created_at: now,
        updated_at: now,
    };
    let gw = harness.node(0).unwrap().gateway();
    gw.persist_time_interval(&always_active).await?;
    gw.upsert_time_interval_cache(always_active)?;
    info!("✓ Registered 'business-hours' interval (always-active stub)\n");

    info!("→ Dispatching alert (active window matches → executes)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed { .. }));
    info!("  ✓ Dispatch executed because interval matches now\n");

    let provider = harness.provider("log").unwrap();
    assert_eq!(provider.call_count(), 1);
    harness.teardown().await?;
    info!("✓ Demo 4 complete\n");

    info!("════════════════════════════════════════════════════════════════");
    info!("  All time interval demos completed successfully");
    info!("════════════════════════════════════════════════════════════════");

    Ok(())
}
