//! Demonstration of `OpenTelemetry` distributed tracing across diverse scenarios.
//!
//! This simulation exercises various dispatch paths to show the span hierarchy
//! that would be emitted when connected to an `OTel` collector (Jaeger, Tempo, etc.).
//! Since simulations run without a real collector, the output describes the
//! expected trace structure and logs timing information for each scenario.
//!
//! Run with: `cargo run -p acteon-simulation --example otel_tracing_simulation`

use std::sync::Arc;
use std::time::{Duration, Instant};

use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::chain::{ChainConfig, ChainStepConfig};
use acteon_core::{Action, ActionOutcome, StateMachineConfig, TransitionConfig};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state::lock::DistributedLock;
use acteon_state::store::StateStore;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

// =============================================================================
// Rule definitions
// =============================================================================

const SUPPRESSION_RULE: &str = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
"#;

const REROUTE_RULE: &str = r#"
rules:
  - name: reroute-urgent-to-sms
    priority: 1
    condition:
      field: action.payload.priority
      eq: "urgent"
    action:
      type: reroute
      target_provider: sms
"#;

const THROTTLE_RULE: &str = r#"
rules:
  - name: throttle-marketing
    priority: 2
    condition:
      field: action.action_type
      eq: "marketing_blast"
    action:
      type: throttle
      max_count: 10
      window_seconds: 60
"#;

const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-notifications
    priority: 3
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

const MODIFY_RULE: &str = r#"
rules:
  - name: enrich-emails
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: modify
      changes:
        tracking_enabled: true
        modified_by: "rule-engine"
"#;

const COMPLEX_RULES: &str = r#"
rules:
  - name: suppress-debug
    priority: 1
    condition:
      field: action.payload.level
      eq: "debug"
    action:
      type: suppress

  - name: escalate-critical
    priority: 2
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: pagerduty

  - name: dedup-warnings
    priority: 5
    condition:
      all:
        - field: action.action_type
          eq: "alert"
        - field: action.payload.severity
          eq: "warning"
    action:
      type: deduplicate
      ttl_seconds: 120

  - name: throttle-info
    priority: 10
    condition:
      all:
        - field: action.action_type
          eq: "alert"
        - field: action.payload.severity
          eq: "info"
    action:
      type: throttle
      max_count: 5
      window_seconds: 30
"#;

const GROUPING_RULE: &str = r#"
rules:
  - name: group-by-cluster
    priority: 10
    condition:
      field: action.action_type
      eq: "metric_alert"
    action:
      type: group
      group_by:
        - tenant
        - payload.cluster
      group_wait_seconds: 30
      group_interval_seconds: 300
      max_group_size: 100
"#;

const STATE_MACHINE_RULE: &str = r#"
rules:
  - name: incident-lifecycle
    priority: 5
    condition:
      field: action.action_type
      eq: "incident"
    action:
      type: state_machine
      state_machine: incident
      fingerprint_fields:
        - action_type
        - payload.incident_id
"#;

const CHAIN_RULE: &str = r#"
rules:
  - name: research-pipeline
    priority: 1
    condition:
      field: action.action_type
      eq: "research"
    action:
      type: chain
      chain: search-summarize-email
"#;

// =============================================================================
// Helpers
// =============================================================================

fn print_header(title: &str) {
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  {title}");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
}

fn print_trace(lines: &[&str]) {
    println!("  Expected OTel trace structure:");
    for line in lines {
        println!("    {line}");
    }
    println!();
}

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

/// Dispatch with timing, printing elapsed and outcome.
async fn timed_dispatch(
    harness: &SimulationHarness,
    action: &Action,
    label: &str,
) -> Result<ActionOutcome, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let outcome = harness.dispatch(action).await?;
    let elapsed = start.elapsed();
    println!("  [{elapsed:>8.2?}] {label}: {outcome:?}");
    Ok(outcome)
}

// =============================================================================
// Scenarios
// =============================================================================

/// Scenario 1: Single action dispatch showing the full span tree with timing.
async fn scenario_basic_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 1: BASIC DISPATCH - FULL SPAN TREE");

    println!("  A single action produces a hierarchy of tracing spans.");
    println!("  With OTel enabled, each span is exported to the collector.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_welcome",
        serde_json::json!({"to": "user@example.com", "subject": "Welcome!"}),
    );
    timed_dispatch(&harness, &action, "send_welcome via email").await?;
    println!(
        "  Provider calls: {}\n",
        harness.provider("email").unwrap().call_count()
    );

    print_trace(&[
        "gateway.dispatch (root span)",
        "  |-- action.id, namespace, tenant, provider, action_type (attributes)",
        "  |-- lock acquisition (distributed lock span)",
        "  |-- rule_engine.evaluate (no rules -> Allow)",
        "  |-- gateway.execute_action",
        "  |   |-- provider = \"email\"",
        "  |   |-- executor.run (retry wrapper)",
        "  |   |   |-- provider.execute (actual provider call)",
        "  |-- audit.record (async background span)",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 2a: Allow, suppress, and reroute verdicts with timing.
async fn scenario_verdicts_allow_suppress_reroute() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 2a: VERDICTS - ALLOW / SUPPRESS / REROUTE");

    println!("  Different rule verdicts produce different child spans.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(SUPPRESSION_RULE)
            .add_rule_yaml(REROUTE_RULE)
            .build(),
    )
    .await?;

    println!("  --- ALLOW (no rule matched) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_receipt",
            serde_json::json!({"order_id": "ORD-100"}),
        ),
        "Allow",
    )
    .await?;
    print_trace(&["gateway.dispatch -> Allow(None) -> execute_action"]);

    println!("  --- SUPPRESS (block-spam) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "notifications",
            "tenant-1",
            "email",
            "spam",
            serde_json::json!({"body": "Buy now!"}),
        ),
        "Suppress",
    )
    .await?;
    print_trace(&["gateway.dispatch -> Suppress(\"block-spam\") [no execute_action]"]);

    println!("  --- REROUTE (reroute-urgent-to-sms) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "notifications",
            "tenant-1",
            "email",
            "alert",
            serde_json::json!({"priority": "urgent", "message": "Server down!"}),
        ),
        "Reroute",
    )
    .await?;
    println!(
        "  SMS calls: {}",
        harness.provider("sms").unwrap().call_count()
    );
    print_trace(&[
        "gateway.dispatch -> Reroute(sms)",
        "  |-- gateway.handle_reroute -> execute_action(provider=sms)",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 2b: Throttle, deduplicate, and modify verdicts.
#[allow(clippy::too_many_lines)]
async fn scenario_verdicts_throttle_dedup_modify() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 2b: VERDICTS - THROTTLE / DEDUPLICATE / MODIFY");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(THROTTLE_RULE)
            .add_rule_yaml(DEDUP_RULE)
            .add_rule_yaml(MODIFY_RULE)
            .build(),
    )
    .await?;

    // Throttle
    println!("  --- THROTTLE (throttle-marketing) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "notifications",
            "tenant-1",
            "email",
            "marketing_blast",
            serde_json::json!({"campaign": "summer-sale"}),
        ),
        "Throttle",
    )
    .await?;
    print_trace(&["gateway.dispatch -> Throttle(window=60s) [no execute_action]"]);

    // Deduplicate
    println!("  --- DEDUPLICATE (dedup-notifications) ---");
    let n1 = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({"message": "New comment"}),
    )
    .with_dedup_key("comment-1");
    timed_dispatch(&harness, &n1, "First (executed)").await?;
    let n2 = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({"message": "New comment"}),
    )
    .with_dedup_key("comment-1");
    timed_dispatch(&harness, &n2, "Duplicate (blocked)").await?;
    print_trace(&[
        "First: handle_dedup -> state.get(miss) -> state.set -> execute_action",
        "Dup:   handle_dedup -> state.get(hit)  -> Deduplicated",
    ]);

    // Modify
    println!("  --- MODIFY (enrich-emails) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({"to": "user@example.com", "body": "Hello"}),
        ),
        "Modify",
    )
    .await?;

    // Verify payload enrichment
    let calls = harness.provider("email").unwrap().calls();
    if let Some(last) = calls.last() {
        let enriched = last.action.payload.get("tracking_enabled").is_some();
        println!("  Payload enriched: tracking_enabled={enriched}");
    }
    print_trace(&[
        "gateway.dispatch -> Modify(enrich-emails)",
        "  |-- json_patch::merge (add tracking_enabled, modified_by)",
        "  |-- gateway.execute_action (modified payload)",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 3: Provider errors with retry spans.
async fn scenario_error_spans() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 3: ERROR SPANS WITH RETRY LOGIC");

    println!("  Provider failures produce error spans. The executor retries");
    println!("  each attempt as a child span.\n");

    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    // Flaky: fails 2x then succeeds
    let flaky =
        Arc::new(FailingProvider::execution_failed("email", "connection reset").fail_until(2));
    let gw = GatewayBuilder::new()
        .state(state.clone() as Arc<dyn StateStore>)
        .lock(lock.clone() as Arc<dyn DistributedLock>)
        .executor_config(ExecutorConfig {
            max_retries: 3,
            ..ExecutorConfig::default()
        })
        .provider(flaky.clone() as Arc<dyn DynProvider>)
        .build()?;

    let start = Instant::now();
    let outcome = gw
        .dispatch(
            Action::new(
                "notifications",
                "tenant-1",
                "email",
                "send_alert",
                serde_json::json!({"message": "Disk full"}),
            ),
            None,
        )
        .await?;
    println!("  [{:>8.2?}] Flaky: {outcome:?}", start.elapsed());
    println!("  Calls: {} (2 fail + 1 ok)\n", flaky.call_count());

    print_trace(&[
        "gateway.dispatch -> Allow -> execute_action",
        "  |-- attempt 1: ERROR (connection reset)",
        "  |-- attempt 2: ERROR (connection reset)",
        "  |-- attempt 3: OK (recovered)",
    ]);

    // Always-fail
    let fail = Arc::new(FailingProvider::execution_failed("webhook", "503"));
    let gw2 = GatewayBuilder::new()
        .state(state as Arc<dyn StateStore>)
        .lock(lock as Arc<dyn DistributedLock>)
        .executor_config(ExecutorConfig {
            max_retries: 2,
            ..ExecutorConfig::default()
        })
        .provider(fail.clone() as Arc<dyn DynProvider>)
        .build()?;

    let start = Instant::now();
    let outcome = gw2
        .dispatch(
            Action::new(
                "webhooks",
                "tenant-1",
                "webhook",
                "notify",
                serde_json::json!({"url": "https://example.com/hook"}),
            ),
            None,
        )
        .await?;
    println!("  [{:>8.2?}] Always-fail: {outcome:?}", start.elapsed());
    println!("  Calls: {} (all failed)\n", fail.call_count());

    print_trace(&[
        "gateway.dispatch -> Allow -> execute_action",
        "  |-- attempts 1-3: ERROR",
        "  |-- otel.status = ERROR, error.message = \"503\"",
    ]);

    gw.shutdown().await;
    gw2.shutdown().await;
    Ok(())
}

/// Scenario 4: 150 concurrent dispatches with overhead measurement.
async fn scenario_high_concurrency() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 4: HIGH CONCURRENCY - 150 CONCURRENT DISPATCHES");

    println!("  Tracing must not degrade throughput. Dispatches 150 actions");
    println!("  concurrently and measures wall-clock time and per-action cost.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let count: u32 = 150;
    let actions: Vec<Action> = (0..count)
        .map(|i| {
            Action::new(
                "bulk",
                "tenant-1",
                "email",
                "bulk_send",
                serde_json::json!({"recipient_id": i, "campaign": "otel-load-test"}),
            )
        })
        .collect();

    let start = Instant::now();
    let outcomes = harness.dispatch_batch(&actions).await;
    let elapsed = start.elapsed();

    let ok = outcomes.iter().filter(|r| r.is_ok()).count();
    let executed = outcomes
        .iter()
        .filter(|r| matches!(r, Ok(ActionOutcome::Executed(_))))
        .count();

    println!("  Total time:     {elapsed:>10.2?}");
    println!("  Successful:     {ok}/{count}");
    println!("  Executed:       {executed}/{count}");
    println!(
        "  Provider calls: {}",
        harness.provider("email").unwrap().call_count()
    );
    let throughput = f64::from(count) / elapsed.as_secs_f64();
    let per_action = elapsed / count;
    println!("  Throughput:     {throughput:.0} actions/sec");
    println!("  Avg per action: {per_action:.2?}\n");

    print_trace(&[
        "150 independent root traces, each with:",
        "  gateway.dispatch -> rule_engine.evaluate -> execute_action",
        "",
        "In a collector waterfall, 150 concurrent timelines appear.",
        "Span overhead should be < 1% of total dispatch latency.",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 5: Complex rule set with 4 priority levels.
#[allow(clippy::too_many_lines)]
async fn scenario_complex_rules() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 5: COMPLEX RULE EVALUATION - MULTI-RULE SET");

    println!("  4 rules at priorities 1, 2, 5, 10. First match wins.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_recording_provider("pagerduty")
            .add_rule_yaml(COMPLEX_RULES)
            .build(),
    )
    .await?;

    println!("  --- suppress-debug (p=1) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"level": "debug", "severity": "low"}),
        ),
        "Suppressed",
    )
    .await?;

    println!("  --- escalate-critical (p=2) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"severity": "critical", "message": "DB down"}),
        ),
        "Rerouted",
    )
    .await?;

    println!("  --- dedup-warnings (p=5) ---");
    let w = Action::new(
        "monitoring",
        "acme",
        "slack",
        "alert",
        serde_json::json!({"severity": "warning", "message": "Latency"}),
    )
    .with_dedup_key("latency-w");
    timed_dispatch(&harness, &w, "1st warning").await?;
    let w2 = Action::new(
        "monitoring",
        "acme",
        "slack",
        "alert",
        serde_json::json!({"severity": "warning", "message": "Latency"}),
    )
    .with_dedup_key("latency-w");
    timed_dispatch(&harness, &w2, "Dup warning").await?;

    println!("  --- throttle-info (p=10) ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"severity": "info", "message": "Deployed"}),
        ),
        "Throttled",
    )
    .await?;

    println!("  --- no match -> Allow ---");
    timed_dispatch(
        &harness,
        &Action::new(
            "monitoring",
            "acme",
            "slack",
            "heartbeat",
            serde_json::json!({"status": "healthy"}),
        ),
        "Allowed",
    )
    .await?;

    println!(
        "\n  Final: Slack={}, PagerDuty={}",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("pagerduty").unwrap().call_count(),
    );

    print_trace(&[
        "Trace for critical escalation:",
        "  gateway.dispatch (action_type=alert)",
        "    |-- rule_engine.evaluate",
        "    |   |-- rule: suppress-debug (p=1, no match)",
        "    |   |-- rule: escalate-critical (p=2, MATCH)",
        "    |   |-- verdict = Reroute(pagerduty)",
        "    |-- gateway.handle_reroute -> execute_action(pagerduty)",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 6: State machine transitions with fingerprint + state spans.
async fn scenario_state_machine() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 6: STATE MACHINE TRANSITIONS");

    println!("  State machine verdicts produce spans for fingerprint");
    println!("  computation, state lookup, and transition validation.\n");

    let sm = StateMachineConfig::new("incident", "open")
        .with_state("open")
        .with_state("acknowledged")
        .with_state("resolved")
        .with_transition(TransitionConfig::new("open", "acknowledged"))
        .with_transition(TransitionConfig::new("acknowledged", "resolved"))
        .with_transition(TransitionConfig::new("open", "resolved"));

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_rule_yaml(STATE_MACHINE_RULE)
            .add_state_machine(sm)
            .build(),
    )
    .await?;

    for (status, msg) in [
        ("open", "API latency > 5s"),
        ("acknowledged", "Engineer investigating"),
        ("resolved", "Root cause fixed"),
    ] {
        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "incident",
            serde_json::json!({"incident_id": "INC-001", "status": status, "message": msg}),
        );
        timed_dispatch(&harness, &action, &format!("state -> {status}")).await?;
    }

    print_trace(&[
        "Each event:",
        "  gateway.dispatch -> StateMachine(incident)",
        "    |-- gateway.handle_state_machine",
        "    |   |-- compute_fingerprint",
        "    |   |-- state.get (current state)",
        "    |   |-- validate_transition",
        "    |   |-- state.set (new state)",
        "    |   |-- execute_action (transition effects)",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 7: Group batching showing group assignment and flush.
async fn scenario_group_batching() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 7: GROUP BATCHING");

    println!("  Group verdicts buffer events. Initial dispatch shows");
    println!("  group assignment; flush is a separate trace.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_rule_yaml(GROUPING_RULE)
            .build(),
    )
    .await?;

    for (source, cluster) in [
        ("pod-restart", "prod-us-east"),
        ("oom-kill", "prod-us-east"),
        ("cpu-throttle", "prod-us-east"),
        ("disk-pressure", "prod-eu-west"),
    ] {
        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "metric_alert",
            serde_json::json!({"cluster": cluster, "source": source, "value": 95.0}),
        );
        timed_dispatch(&harness, &action, &format!("{source}@{cluster}")).await?;
    }

    println!(
        "\n  Provider calls: {} (buffered, not flushed)",
        harness.provider("slack").unwrap().call_count()
    );

    print_trace(&[
        "Each group dispatch:",
        "  gateway.dispatch -> Group(group_by=[tenant,cluster])",
        "    |-- gateway.handle_group -> group_manager.add -> Grouped",
        "",
        "On flush (after group_wait_seconds=30):",
        "  gateway.flush_group (separate trace)",
        "    |-- group_manager.flush -> execute_action (batch)",
        "",
        "Groups: prod-us-east (3 events), prod-eu-west (1 event)",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 8: Multi-step chain showing step-by-step span progression.
async fn scenario_chain_tracing() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 8: CHAIN TRACING - MULTI-STEP PIPELINE");

    println!("  Chain verdicts start an async multi-step pipeline. Each");
    println!("  step advance produces its own trace linked to the chain.\n");

    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let search = Arc::new(RecordingProvider::new("search-api").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "results": [{"title": "Rust primer", "url": "https://example.com/1"}]
        })))
    }));
    let summarize = Arc::new(RecordingProvider::new("llm-api").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "summary": "Rust uses async/await with Tokio."
        })))
    }));
    let email = Arc::new(RecordingProvider::new("email"));

    let chain = ChainConfig::new("search-summarize-email")
        .with_step(ChainStepConfig::new(
            "search",
            "search-api",
            "web_search",
            serde_json::json!({"query": "{{origin.payload.query}}"}),
        ))
        .with_step(ChainStepConfig::new(
            "summarize",
            "llm-api",
            "summarize",
            serde_json::json!({"text": "{{prev.response_body}}", "max_length": 200}),
        ))
        .with_step(ChainStepConfig::new(
            "notify",
            "email",
            "send_email",
            serde_json::json!({
                "to": "{{origin.payload.email}}",
                "subject": "Results for: {{origin.payload.query}}",
                "body": "{{prev.response_body.summary}}"
            }),
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::clone(&search) as Arc<dyn DynProvider>)
        .provider(Arc::clone(&summarize) as Arc<dyn DynProvider>)
        .provider(Arc::clone(&email) as Arc<dyn DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let start = Instant::now();
    let outcome = gateway
        .dispatch(
            Action::new(
                "research",
                "tenant-1",
                "search-api",
                "research",
                serde_json::json!({"query": "rust async", "email": "dev@example.com"}),
            ),
            None,
        )
        .await?;
    println!("  [{:>8.2?}] Chain dispatch: {outcome:?}", start.elapsed());

    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            total_steps,
            ..
        } => {
            println!("  Chain: {chain_name}, steps: {total_steps}, id: {chain_id}");
            chain_id.clone()
        }
        other => {
            println!("  Unexpected: {other:?}");
            gateway.shutdown().await;
            return Ok(());
        }
    };

    // Advance through all 3 steps
    for step in 1..=3 {
        let step_start = Instant::now();
        gateway
            .advance_chain("research", "tenant-1", &chain_id)
            .await?;
        println!("  [{:>8.2?}] Step {step} advanced", step_start.elapsed());
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    println!(
        "\n  Provider calls: search-api={}, llm-api={}, email={}",
        search.call_count(),
        summarize.call_count(),
        email.call_count()
    );

    if let Some(cs) = gateway
        .get_chain_status("research", "tenant-1", &chain_id)
        .await?
    {
        println!("  Chain status: {:?}", cs.status);
    }

    print_trace(&[
        "Initial dispatch:",
        "  gateway.dispatch -> Chain(search-summarize-email)",
        "    |-- gateway.handle_chain -> state.set -> ChainStarted",
        "",
        "Each advance_chain (separate trace):",
        "  gateway.advance_chain (chain_id, step_index)",
        "    |-- state.get (load chain state)",
        "    |-- template rendering (payload interpolation)",
        "    |-- gateway.execute_action (step provider)",
        "    |-- state.set (update step result)",
        "    |-- audit.record (step outcome)",
    ]);

    gateway.shutdown().await;
    Ok(())
}

/// Scenario 9: Batch dispatch with mixed verdicts and timing.
async fn scenario_batch_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 9: BATCH DISPATCH WITH MIXED VERDICTS");

    println!("  Batch dispatch sends multiple actions. Each action gets");
    println!("  its own trace. Timing shows batch overhead.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(SUPPRESSION_RULE)
            .add_rule_yaml(REROUTE_RULE)
            .build(),
    )
    .await?;

    let mut actions = Vec::new();
    for i in 0..10 {
        let (action_type, payload) = match i % 3 {
            0 => ("send_receipt", serde_json::json!({"order_id": i})),
            1 => ("spam", serde_json::json!({"body": "spam"})),
            _ => ("alert", serde_json::json!({"priority": "urgent"})),
        };
        actions.push(Action::new("ns", "t1", "email", action_type, payload));
    }

    let start = Instant::now();
    let outcomes = harness.dispatch_batch(&actions).await;
    let elapsed = start.elapsed();

    let mut allowed = 0;
    let mut suppressed = 0;
    let mut rerouted = 0;
    for result in &outcomes {
        match result {
            Ok(ActionOutcome::Executed(_)) => allowed += 1,
            Ok(ActionOutcome::Suppressed { .. }) => suppressed += 1,
            Ok(ActionOutcome::Rerouted { .. }) => rerouted += 1,
            _ => {}
        }
    }

    println!("  Batch of 10 in {elapsed:.2?}:");
    println!("    Allowed:    {allowed}");
    println!("    Suppressed: {suppressed}");
    println!("    Rerouted:   {rerouted}");
    println!("    Avg/action: {:.2?}\n", elapsed / 10);

    print_trace(&[
        "10 independent traces with mixed verdicts:",
        "  4x Allow -> execute_action(email)",
        "  3x Suppress(block-spam)",
        "  3x Reroute(sms) -> execute_action(sms)",
        "",
        "No cross-trace linking in batch; each is independent.",
    ]);

    harness.teardown().await?;
    Ok(())
}

/// Scenario 10: Graceful behavior without an `OTel` collector.
async fn scenario_no_collector() -> Result<(), Box<dyn std::error::Error>> {
    print_header("SCENARIO 10: NO COLLECTOR - GRACEFUL DEGRADATION");

    println!("  When OTel is disabled (the default), tracing spans are");
    println!("  created via the `tracing` crate but not exported. This");
    println!("  ensures zero overhead without a collector.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    timed_dispatch(
        &harness,
        &Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({"to": "user@example.com"}),
        ),
        "No collector",
    )
    .await?;

    println!("\n  OTel disabled: no exporter overhead, spans only in fmt logs.");
    println!("  Enable with [telemetry] enabled = true in acteon.toml.");
    println!();
    println!("  Note: Approval workflow tracing and cross-service context");
    println!("  propagation (traceparent/tracestate headers) are HTTP-layer");
    println!("  features. See these simulations for those scenarios:");
    println!("    - approval_simulation.rs (requires running server)");
    println!("    - http_client_simulation.rs (requires running server)");

    harness.teardown().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║        OPENTELEMETRY TRACING SIMULATION DEMO                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  Exercises diverse dispatch paths showing the OTel span");
    println!("  hierarchy. No collector needed -- output describes traces");
    println!("  and logs per-operation timing.");

    scenario_basic_dispatch().await?;
    scenario_verdicts_allow_suppress_reroute().await?;
    scenario_verdicts_throttle_dedup_modify().await?;
    scenario_error_spans().await?;
    scenario_high_concurrency().await?;
    scenario_complex_rules().await?;
    scenario_state_machine().await?;
    scenario_group_batching().await?;
    scenario_chain_tracing().await?;
    scenario_batch_dispatch().await?;
    scenario_no_collector().await?;

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║              OTEL TRACING SIMULATION COMPLETE                ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  To see real traces, enable telemetry in acteon.toml:");
    println!("    [telemetry]");
    println!("    enabled = true");
    println!("    endpoint = \"http://localhost:4317\"");
    println!("    sample_ratio = 1.0");

    Ok(())
}
