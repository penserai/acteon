//! Demonstration of circuit breaker behavior in Acteon.
//!
//! This simulation shows how circuit breakers protect against cascading
//! failures by automatically stopping requests to unhealthy providers
//! and optionally rerouting traffic to a fallback.
//!
//! Run with: cargo run -p acteon-simulation --example circuit_breaker_simulation

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome};
use acteon_gateway::{CircuitBreakerConfig, GatewayBuilder};
use acteon_simulation::provider::{FailureMode, RecordingProvider};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

/// Helper to build a gateway with circuit breaker configuration.
/// Accepts providers and an optional per-provider circuit breaker config map.
async fn build_gateway(
    providers: Vec<Arc<RecordingProvider>>,
    cb_default: CircuitBreakerConfig,
    cb_overrides: Vec<(String, CircuitBreakerConfig)>,
) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .circuit_breaker(cb_default);

    for (name, config) in cb_overrides {
        builder = builder.circuit_breaker_provider(name, config);
    }

    for p in providers {
        builder = builder.provider(p as Arc<dyn acteon_provider::DynProvider>);
    }

    builder.build().expect("gateway should build")
}

fn make_action(provider: &str) -> Action {
    Action::new(
        "simulation",
        "tenant-1",
        provider,
        "send_notification",
        serde_json::json!({
            "to": "user@example.com",
            "message": "Hello from circuit breaker simulation",
        }),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          CIRCUIT BREAKER SIMULATION DEMO                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Basic Circuit Opening
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: BASIC CIRCUIT OPENING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A provider that always fails will cause the circuit to open");
    println!("  after reaching the failure threshold (3 failures).\n");

    let email = Arc::new(RecordingProvider::new("email").with_failure_mode(FailureMode::Always));

    let gateway = build_gateway(
        vec![Arc::clone(&email)],
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(3600), // Long timeout so circuit stays open
            fallback_provider: None,
        },
        vec![],
    )
    .await;

    // Send actions that will fail and eventually trip the circuit
    for i in 1..=5 {
        let action = make_action("email");
        let outcome = gateway.dispatch(action, None).await?;
        let cb_state = gateway
            .circuit_breakers()
            .unwrap()
            .get("email")
            .unwrap()
            .state();
        match &outcome {
            ActionOutcome::Failed(err) => {
                println!(
                    "  Request {i}: FAILED (error: {}) | Circuit: {cb_state}",
                    err.message
                );
            }
            ActionOutcome::CircuitOpen { provider, .. } => {
                println!(
                    "  Request {i}: CIRCUIT OPEN (provider: {provider}) | Circuit: {cb_state}"
                );
            }
            other => {
                println!("  Request {i}: {:?} | Circuit: {cb_state}", other);
            }
        }

        if i == 3 {
            println!("  --- Circuit opened after {i} consecutive failures ---");
        }
    }

    // Verify: first 3 calls hit the provider (and failed), last 2 were rejected
    // immediately by the circuit breaker without calling the provider.
    assert_eq!(
        email.call_count(),
        3,
        "provider should be called exactly 3 times"
    );
    println!("\n  Provider was called 3 times (requests 1-3).");
    println!("  Requests 4-5 were rejected immediately by the circuit breaker.");

    let snap = gateway.metrics().snapshot();
    println!("  Circuit-open rejections: {}", snap.circuit_open);

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Fallback Routing
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: FALLBACK ROUTING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  When the primary provider's circuit opens and a fallback is");
    println!("  configured, traffic is automatically rerouted to the fallback.\n");

    let primary =
        Arc::new(RecordingProvider::new("primary").with_failure_mode(FailureMode::Always));
    let fallback = Arc::new(RecordingProvider::new("fallback"));

    let gateway = build_gateway(
        vec![Arc::clone(&primary), Arc::clone(&fallback)],
        CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None, // Default: no fallback
        },
        vec![(
            "primary".to_string(),
            CircuitBreakerConfig {
                failure_threshold: 2,
                success_threshold: 1,
                recovery_timeout: Duration::from_secs(3600),
                fallback_provider: Some("fallback".to_string()),
            },
        )],
    )
    .await;

    // Trip the circuit with 2 failures
    for i in 1..=2 {
        let action = make_action("primary");
        let outcome = gateway.dispatch(action, None).await?;
        println!("  Request {i}: {:?}", outcome_summary(&outcome));
    }
    println!("  --- Circuit opened after 2 failures ---\n");

    // Now send more requests -- they should be rerouted to fallback
    for i in 3..=5 {
        let action = make_action("primary");
        let outcome = gateway.dispatch(action, None).await?;
        match &outcome {
            ActionOutcome::Rerouted {
                original_provider,
                new_provider,
                ..
            } => {
                println!("  Request {i}: REROUTED from '{original_provider}' to '{new_provider}'");
            }
            other => {
                println!("  Request {i}: {:?}", outcome_summary(other));
            }
        }
    }

    assert_eq!(
        primary.call_count(),
        2,
        "primary called only during closed state"
    );
    assert_eq!(
        fallback.call_count(),
        3,
        "fallback received all rerouted traffic"
    );
    println!(
        "\n  Primary provider calls: {} | Fallback provider calls: {}",
        primary.call_count(),
        fallback.call_count()
    );
    let snap = gateway.metrics().snapshot();
    println!("  Circuit-fallback reroutes: {}", snap.circuit_fallbacks);

    gateway.shutdown().await;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Full Recovery Lifecycle
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: FULL RECOVERY LIFECYCLE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Demonstrates the full circuit breaker lifecycle:");
    println!("  Closed -> Open -> HalfOpen -> Closed\n");

    // Use FirstN to simulate a provider that fails initially then recovers.
    // With FirstN(3), calls 1-3 fail, calls 4+ succeed.
    let recovering =
        Arc::new(RecordingProvider::new("recovering").with_failure_mode(FailureMode::FirstN(3)));

    let gateway = build_gateway(
        vec![Arc::clone(&recovering)],
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::ZERO, // Instant recovery for demo
            fallback_provider: None,
        },
        vec![],
    )
    .await;

    let cb_registry = gateway.circuit_breakers().unwrap();
    let cb = cb_registry.get("recovering").unwrap();

    // Phase 1: Closed -> Open (3 consecutive failures)
    println!("  Phase 1: CLOSED -> OPEN");
    for i in 1..=3 {
        let action = make_action("recovering");
        let outcome = gateway.dispatch(action, None).await?;
        println!(
            "    Request {i}: {} | Circuit: {}",
            outcome_summary(&outcome),
            cb.state()
        );
    }
    assert_eq!(
        cb.state().to_string(),
        "open",
        "circuit should be open after 3 failures"
    );
    println!("    -> Circuit is now OPEN\n");

    // Phase 2: Open -> HalfOpen (recovery_timeout is ZERO, so check() transitions immediately)
    println!("  Phase 2: OPEN -> HALF-OPEN -> CLOSED");
    println!("    (recovery_timeout=0s, so transition is immediate)");

    // The next dispatch will trigger check() which transitions Open->HalfOpen,
    // then the provider succeeds (call #4, which is past FirstN(3)),
    // recording a success in HalfOpen state.
    let action = make_action("recovering");
    let outcome = gateway.dispatch(action, None).await?;
    println!(
        "    Request 4: {} | Circuit: {}",
        outcome_summary(&outcome),
        cb.state()
    );

    // One more success to meet success_threshold=2
    let action = make_action("recovering");
    let outcome = gateway.dispatch(action, None).await?;
    println!(
        "    Request 5: {} | Circuit: {}",
        outcome_summary(&outcome),
        cb.state()
    );

    assert_eq!(
        cb.state().to_string(),
        "closed",
        "circuit should be closed after 2 successes in half-open"
    );
    println!("    -> Circuit is now CLOSED (recovered!)\n");

    // Phase 3: Verify normal operation resumes
    println!("  Phase 3: Normal operation resumed");
    let action = make_action("recovering");
    let outcome = gateway.dispatch(action, None).await?;
    assert!(outcome.is_executed(), "should execute normally");
    println!(
        "    Request 6: {} | Circuit: {}",
        outcome_summary(&outcome),
        cb.state()
    );

    println!("\n  Total provider calls: {}", recovering.call_count());

    gateway.shutdown().await;
    println!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // SCENARIO 4: Independent Circuits Per Provider
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: INDEPENDENT CIRCUITS PER PROVIDER");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Each provider has its own circuit breaker. One provider failing");
    println!("  does not affect other providers.\n");

    let email_provider =
        Arc::new(RecordingProvider::new("email").with_failure_mode(FailureMode::Always));
    let sms_provider = Arc::new(RecordingProvider::new("sms")); // healthy
    let webhook_provider =
        Arc::new(RecordingProvider::new("webhook").with_failure_mode(FailureMode::FirstN(1)));

    let gateway = build_gateway(
        vec![
            Arc::clone(&email_provider),
            Arc::clone(&sms_provider),
            Arc::clone(&webhook_provider),
        ],
        CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        },
        vec![],
    )
    .await;

    let cb_registry = gateway.circuit_breakers().unwrap();

    // Trip the email circuit (2 failures)
    println!("  Sending 2 requests to 'email' (always fails)...");
    for _ in 0..2 {
        let action = make_action("email");
        let _ = gateway.dispatch(action, None).await?;
    }
    let email_state = cb_registry.get("email").unwrap().state();
    println!("    email circuit: {email_state}");

    // SMS should still be working fine
    println!("\n  Sending 2 requests to 'sms' (healthy)...");
    for _ in 0..2 {
        let action = make_action("sms");
        let outcome = gateway.dispatch(action, None).await?;
        assert!(outcome.is_executed(), "sms should execute normally");
    }
    let sms_state = cb_registry.get("sms").unwrap().state();
    println!("    sms circuit: {sms_state}");

    // Webhook had 1 failure (FirstN(1)), then succeeded -- still closed
    println!("\n  Sending 2 requests to 'webhook' (fails first, then recovers)...");
    for _ in 0..2 {
        let action = make_action("webhook");
        let _ = gateway.dispatch(action, None).await?;
    }
    let webhook_state = cb_registry.get("webhook").unwrap().state();
    println!("    webhook circuit: {webhook_state}");

    // Verify states
    assert_eq!(email_state.to_string(), "open", "email should be open");
    assert_eq!(sms_state.to_string(), "closed", "sms should be closed");
    assert_eq!(
        webhook_state.to_string(),
        "closed",
        "webhook should be closed (failure + success resets counter)"
    );

    // Now try to send to email -- should be rejected
    let action = make_action("email");
    let outcome = gateway.dispatch(action, None).await?;
    assert!(
        matches!(outcome, ActionOutcome::CircuitOpen { .. }),
        "email should be circuit-open"
    );
    println!("\n  Email request after circuit open: REJECTED (CircuitOpen)");

    // SMS still works
    let action = make_action("sms");
    let outcome = gateway.dispatch(action, None).await?;
    assert!(outcome.is_executed(), "sms should still work");
    println!("  SMS request: EXECUTED (unaffected by email's circuit)");

    println!(
        "\n  Final call counts: email={}, sms={}, webhook={}",
        email_provider.call_count(),
        sms_provider.call_count(),
        webhook_provider.call_count(),
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 4 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              ALL SCENARIOS PASSED                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Produce a short human-readable summary of an `ActionOutcome`.
fn outcome_summary(outcome: &ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Executed(_) => "EXECUTED".to_string(),
        ActionOutcome::Failed(err) => format!("FAILED ({})", err.message),
        ActionOutcome::CircuitOpen { provider, .. } => {
            format!("CIRCUIT_OPEN (provider: {provider})")
        }
        ActionOutcome::Rerouted {
            original_provider,
            new_provider,
            ..
        } => format!("REROUTED ({original_provider} -> {new_provider})"),
        ActionOutcome::Suppressed { rule } => format!("SUPPRESSED (rule: {rule})"),
        ActionOutcome::Deduplicated => "DEDUPLICATED".to_string(),
        other => format!("{other:?}"),
    }
}

// Convenience trait import for `.is_executed()`, etc.
use acteon_simulation::assertions::ActionOutcomeExt as _;
