//! Simulation of chain step retry policies.
//!
//! Demonstrates:
//! 1. Retry succeeds on second attempt (provider fails once, then succeeds)
//! 2. Retry exhausted with abort policy (chain fails after max retries)
//! 3. Retry exhausted with skip policy (chain continues past failed step)
//! 4. Exponential backoff (verify delays increase between attempts)
//!
//! Run with: `cargo run -p acteon-simulation --example retry_chain_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_audit::record::AuditQuery;
use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::chain::{
    ChainConfig, ChainStepConfig, RetryBackoffStrategy, RetryPolicy, StepFailurePolicy,
};
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use tracing::info;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

fn chain_rule(action_type: &str, chain_name: &str) -> String {
    format!(
        r#"
rules:
  - name: trigger-{chain_name}
    priority: 1
    condition:
      field: action.action_type
      eq: "{action_type}"
    action:
      type: chain
      chain: {chain_name}
"#
    )
}

fn extract_chain_id(outcome: &ActionOutcome) -> String {
    match outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("expected ChainStarted, got {other:?}"),
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("==================================================================");
    info!("           ACTEON CHAIN RETRY SIMULATION");
    info!("==================================================================\n");

    // =========================================================================
    // DEMO 1: Retry Succeeds on Second Attempt
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 1: RETRY SUCCEEDS ON SECOND ATTEMPT");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    // Provider fails the first call, then succeeds on subsequent calls.
    let flaky_provider =
        Arc::new(RecordingProvider::new("flaky-svc").with_failure_mode(FailureMode::FirstN(1)));
    let final_provider = Arc::new(RecordingProvider::new("final-svc"));

    let chain_config = ChainConfig::new("retry-succeed")
        .with_step(
            ChainStepConfig::new("flaky-step", "flaky-svc", "do_work", serde_json::json!({}))
                .with_retry(RetryPolicy {
                    max_retries: 2,
                    backoff_ms: 10,
                    strategy: RetryBackoffStrategy::Fixed,
                    jitter_ms: None,
                }),
        )
        .with_step(ChainStepConfig::new(
            "final-step",
            "final-svc",
            "finish",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rules = parse_rules(&chain_rule("trigger_retry_ok", "retry-succeed"));

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&flaky_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&final_provider) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "ns",
        "tenant-1",
        "flaky-svc",
        "trigger_retry_ok",
        serde_json::json!({}),
    );

    info!("  Dispatching action...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = extract_chain_id(&outcome);
    info!("  Chain started: {chain_id}");

    // First advance: step 0 attempt 1 fails, retry is scheduled.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Step 0 attempt 1: failed (expected), retry scheduled");

    // Small sleep to respect the 10ms backoff.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Second advance: step 0 attempt 2 succeeds.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Step 0 attempt 2: succeeded");

    // Third advance: step 1 succeeds.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Step 1: succeeded");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let chain_state = gateway
        .get_chain_status("ns", "tenant-1", &chain_id)
        .await?
        .expect("chain should exist");
    info!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed,
        "chain should complete after retry succeeds"
    );

    // Flaky provider was called twice (1 failure + 1 success).
    info!("  flaky-svc calls: {}", flaky_provider.call_count());
    info!("  final-svc calls: {}", final_provider.call_count());
    assert_eq!(flaky_provider.call_count(), 2, "flaky-svc: 1 fail + 1 ok");
    assert_eq!(final_provider.call_count(), 1, "final-svc: 1 ok");

    // Verify step_history records both attempts for step 0.
    assert_eq!(
        chain_state.step_history[0].len(),
        2,
        "step 0 should have 2 attempt records"
    );
    assert!(!chain_state.step_history[0][0].success, "attempt 1 failed");
    assert!(
        chain_state.step_history[0][1].success,
        "attempt 2 succeeded"
    );

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 2: Retry Exhausted — Abort
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 2: RETRY EXHAUSTED — ABORT POLICY");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    // Provider always fails with a retryable error.
    let always_fail = Arc::new(FailingProvider::connection_error(
        "always-fail",
        "service unavailable",
    ));
    let unreachable = Arc::new(RecordingProvider::new("after-fail"));

    let chain_config = ChainConfig::new("retry-exhaust-abort")
        .with_step(
            ChainStepConfig::new(
                "unreliable",
                "always-fail",
                "call_svc",
                serde_json::json!({}),
            )
            .with_retry(RetryPolicy {
                max_retries: 1,
                backoff_ms: 10,
                strategy: RetryBackoffStrategy::Fixed,
                jitter_ms: None,
            }),
            // on_failure defaults to Abort
        )
        .with_step(ChainStepConfig::new(
            "should-not-run",
            "after-fail",
            "noop",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rules = parse_rules(&chain_rule("trigger_exhaust_abort", "retry-exhaust-abort"));

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&always_fail) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&unreachable) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "ns",
        "tenant-1",
        "always-fail",
        "trigger_exhaust_abort",
        serde_json::json!({}),
    );

    info!("  Dispatching action...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = extract_chain_id(&outcome);

    // Attempt 1: fails, retry scheduled.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Attempt 1: failed, retry scheduled");

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Attempt 2: fails again, retries exhausted -> abort.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Attempt 2: failed, retries exhausted -> chain aborted");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let chain_state = gateway
        .get_chain_status("ns", "tenant-1", &chain_id)
        .await?
        .expect("chain should exist");
    info!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Failed,
        "chain should fail after retries exhausted"
    );

    info!(
        "  always-fail calls: {} (1 initial + 1 retry)",
        always_fail.call_count()
    );
    assert_eq!(always_fail.call_count(), 2, "1 initial + 1 retry");
    unreachable.assert_not_called();
    info!("  after-fail calls: 0 (never reached)");

    // Verify audit has retry records.
    let page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id.clone()),
            ..Default::default()
        })
        .await?;
    info!("\n  Audit records: {}", page.total);
    for rec in &page.records {
        info!("    [{:>22}] {}", rec.outcome, rec.action_type);
    }
    let failed_terminal: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.outcome == "chain_failed")
        .collect();
    assert_eq!(failed_terminal.len(), 1, "expected 1 chain_failed record");

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 3: Retry Exhausted — Skip
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 3: RETRY EXHAUSTED — SKIP POLICY");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let always_fail = Arc::new(FailingProvider::connection_error(
        "skip-fail",
        "service unavailable",
    ));
    let after_skip = Arc::new(RecordingProvider::new("after-skip").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"done": true}),
        ))
    }));

    let chain_config = ChainConfig::new("retry-exhaust-skip")
        .with_step(
            ChainStepConfig::new(
                "optional-step",
                "skip-fail",
                "call_svc",
                serde_json::json!({}),
            )
            .with_retry(RetryPolicy {
                max_retries: 1,
                backoff_ms: 10,
                strategy: RetryBackoffStrategy::Fixed,
                jitter_ms: None,
            })
            .with_on_failure(StepFailurePolicy::Skip),
        )
        .with_step(ChainStepConfig::new(
            "continue-step",
            "after-skip",
            "finish",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rules = parse_rules(&chain_rule("trigger_exhaust_skip", "retry-exhaust-skip"));

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&always_fail) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&after_skip) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "ns",
        "tenant-1",
        "skip-fail",
        "trigger_exhaust_skip",
        serde_json::json!({}),
    );

    info!("  Dispatching action...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = extract_chain_id(&outcome);

    // Attempt 1: fails, retry scheduled.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Attempt 1: failed, retry scheduled");

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Attempt 2: fails again, retries exhausted -> skip.
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Attempt 2: failed, retries exhausted -> step skipped");

    // Advance step 1 (continue-step).
    gateway.advance_chain("ns", "tenant-1", &chain_id).await?;
    info!("  Step 1 (continue-step): succeeded");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let chain_state = gateway
        .get_chain_status("ns", "tenant-1", &chain_id)
        .await?
        .expect("chain should exist");
    info!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed,
        "chain should complete after skip"
    );

    info!("  skip-fail calls: {}", always_fail.call_count());
    info!("  after-skip calls: {}", after_skip.call_count());
    assert_eq!(always_fail.call_count(), 2, "1 initial + 1 retry");
    assert_eq!(after_skip.call_count(), 1, "continue-step executed");

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 4: Exponential Backoff Delay Verification
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 4: EXPONENTIAL BACKOFF DELAYS");
    info!("------------------------------------------------------------------\n");

    // Verify delays computed by RetryPolicy directly (no gateway needed).
    let policy = RetryPolicy {
        max_retries: 4,
        backoff_ms: 100,
        strategy: RetryBackoffStrategy::Exponential,
        jitter_ms: None,
    };

    let delay1 = policy.compute_delay_ms(1); // 100 * 2^0 = 100
    let delay2 = policy.compute_delay_ms(2); // 100 * 2^1 = 200
    let delay3 = policy.compute_delay_ms(3); // 100 * 2^2 = 400
    let delay4 = policy.compute_delay_ms(4); // 100 * 2^3 = 800

    info!("  Exponential backoff (base=100ms):");
    info!("    Attempt 1 delay: {delay1}ms (expected 100)");
    info!("    Attempt 2 delay: {delay2}ms (expected 200)");
    info!("    Attempt 3 delay: {delay3}ms (expected 400)");
    info!("    Attempt 4 delay: {delay4}ms (expected 800)");

    assert_eq!(delay1, 100, "attempt 1: 100 * 2^0");
    assert_eq!(delay2, 200, "attempt 2: 100 * 2^1");
    assert_eq!(delay3, 400, "attempt 3: 100 * 2^2");
    assert_eq!(delay4, 800, "attempt 4: 100 * 2^3");

    // Verify each delay is strictly greater than the previous.
    assert!(delay2 > delay1, "delay must increase");
    assert!(delay3 > delay2, "delay must increase");
    assert!(delay4 > delay3, "delay must increase");

    // Also verify fixed and linear for completeness.
    let fixed = RetryPolicy {
        max_retries: 3,
        backoff_ms: 500,
        strategy: RetryBackoffStrategy::Fixed,
        jitter_ms: None,
    };
    assert_eq!(fixed.compute_delay_ms(1), 500);
    assert_eq!(fixed.compute_delay_ms(2), 500);
    assert_eq!(fixed.compute_delay_ms(3), 500);
    info!("\n  Fixed backoff: 500ms, 500ms, 500ms (constant)");

    let linear = RetryPolicy {
        max_retries: 3,
        backoff_ms: 100,
        strategy: RetryBackoffStrategy::Linear,
        jitter_ms: None,
    };
    assert_eq!(linear.compute_delay_ms(1), 100);
    assert_eq!(linear.compute_delay_ms(2), 200);
    assert_eq!(linear.compute_delay_ms(3), 300);
    info!("  Linear backoff:  100ms, 200ms, 300ms");

    // Verify jitter adds half the jitter value (deterministic in compute_delay_ms).
    let with_jitter = RetryPolicy {
        max_retries: 2,
        backoff_ms: 100,
        strategy: RetryBackoffStrategy::Fixed,
        jitter_ms: Some(50),
    };
    let jittered = with_jitter.compute_delay_ms(1);
    info!("  Fixed + 50ms jitter: {jittered}ms (expected 125)");
    assert_eq!(jittered, 125, "100 + 50/2 jitter");

    info!("\n  PASSED\n");

    info!("==================================================================");
    info!("           ALL RETRY SIMULATIONS PASSED");
    info!("==================================================================");

    Ok(())
}
