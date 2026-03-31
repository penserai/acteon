//! Demonstration of provider health tracking and metrics in Acteon.
//!
//! This simulation shows how the gateway tracks per-provider execution
//! statistics including success rates, latency percentiles, and last errors.
//!
//! Run with: cargo run -p acteon-simulation --example provider_health_simulation

use std::sync::Arc;
use std::time::Duration;

use acteon_core::Action;
use acteon_executor::ExecutorConfig;
use acteon_gateway::{CircuitBreakerConfig, GatewayBuilder};
use acteon_simulation::provider::{FailureMode, RecordingProvider};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use tracing::info;

/// Helper to build a gateway with providers.
async fn build_gateway(providers: Vec<Arc<RecordingProvider>>) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .executor_config(ExecutorConfig {
            max_retries: 0,
            ..ExecutorConfig::default()
        })
        .circuit_breaker(CircuitBreakerConfig {
            failure_threshold: 1000, // High threshold to avoid interfering with tests
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        });

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
            "message": "Hello from provider health simulation",
        }),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║       PROVIDER HEALTH DASHBOARD SIMULATION DEMO            ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Healthy Providers
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: HEALTHY PROVIDERS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Multiple providers processing actions successfully.");
    info!("  All should show 100% success rate and low latency.\n");

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack"));
    let webhook = Arc::new(RecordingProvider::new("webhook"));

    let gateway = build_gateway(vec![
        Arc::clone(&email),
        Arc::clone(&slack),
        Arc::clone(&webhook),
    ])
    .await;

    // Send actions to each provider
    for _ in 0..10 {
        gateway.dispatch(make_action("email"), None).await?;
        gateway.dispatch(make_action("slack"), None).await?;
        gateway.dispatch(make_action("webhook"), None).await?;
    }

    // Verify metrics
    let metrics = gateway.provider_metrics().snapshot();
    info!("  Provider Health Metrics:");
    for (name, stats) in &metrics {
        info!(
            "    {}: {} requests, {:.1}% success, avg {:.2}ms (p50: {:.2}ms, p95: {:.2}ms, p99: {:.2}ms)",
            name,
            stats.total_requests,
            stats.success_rate,
            stats.avg_latency_ms,
            stats.p50_latency_ms,
            stats.p95_latency_ms,
            stats.p99_latency_ms
        );
        assert_eq!(stats.total_requests, 10);
        assert_eq!(stats.successes, 10);
        assert_eq!(stats.failures, 0);
        assert!((stats.success_rate - 100.0).abs() < f64::EPSILON);
        assert!(stats.last_error.is_none());
    }

    gateway.shutdown().await;
    info!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Degraded Provider
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: DEGRADED PROVIDER");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  One provider has ~30% failure rate.");
    info!("  Metrics should reflect the degraded state.\n");

    // EveryN(3) means every 3rd call fails, giving us ~33% failure rate
    let degraded = Arc::new(
        RecordingProvider::new("degraded-provider").with_failure_mode(FailureMode::EveryN(3)),
    );
    let healthy = Arc::new(RecordingProvider::new("healthy-provider"));

    let gateway = build_gateway(vec![Arc::clone(&degraded), Arc::clone(&healthy)]).await;

    // Send 30 requests to each provider
    for _ in 0..30 {
        let _ = gateway
            .dispatch(make_action("degraded-provider"), None)
            .await;
        gateway
            .dispatch(make_action("healthy-provider"), None)
            .await?;
    }

    // Check metrics
    let degraded_stats = gateway
        .provider_metrics()
        .snapshot_for("degraded-provider")
        .unwrap();
    let healthy_stats = gateway
        .provider_metrics()
        .snapshot_for("healthy-provider")
        .unwrap();

    info!("  Degraded Provider:");
    info!(
        "    Requests: {}, Success Rate: {:.1}%, Failures: {}",
        degraded_stats.total_requests, degraded_stats.success_rate, degraded_stats.failures
    );
    info!(
        "    Last Error: {:?}",
        degraded_stats.last_error.as_deref().unwrap_or("none")
    );

    info!("\n  Healthy Provider:");
    info!(
        "    Requests: {}, Success Rate: {:.1}%, Failures: {}",
        healthy_stats.total_requests, healthy_stats.success_rate, healthy_stats.failures
    );

    // Verify degraded provider has failures
    assert_eq!(degraded_stats.total_requests, 30);
    assert!(degraded_stats.failures >= 9 && degraded_stats.failures <= 11); // ~33%
    assert!(degraded_stats.success_rate >= 60.0 && degraded_stats.success_rate <= 70.0);
    assert!(degraded_stats.last_error.is_some());

    // Verify healthy provider is unaffected
    assert_eq!(healthy_stats.total_requests, 30);
    assert_eq!(healthy_stats.successes, 30);
    assert!((healthy_stats.success_rate - 100.0).abs() < f64::EPSILON);

    gateway.shutdown().await;
    info!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: High Latency Provider
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: HIGH LATENCY PROVIDER");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  One provider has artificial delay added.");
    info!("  Latency percentiles should reflect the delay distribution.\n");

    let slow =
        Arc::new(RecordingProvider::new("slow-provider").with_delay(Duration::from_millis(100)));
    let fast = Arc::new(RecordingProvider::new("fast-provider"));

    let gateway = build_gateway(vec![Arc::clone(&slow), Arc::clone(&fast)]).await;

    // Send requests
    for _ in 0..20 {
        gateway.dispatch(make_action("slow-provider"), None).await?;
        gateway.dispatch(make_action("fast-provider"), None).await?;
    }

    let slow_stats = gateway
        .provider_metrics()
        .snapshot_for("slow-provider")
        .unwrap();
    let fast_stats = gateway
        .provider_metrics()
        .snapshot_for("fast-provider")
        .unwrap();

    info!("  Slow Provider (100ms delay):");
    info!(
        "    Avg: {:.2}ms, p50: {:.2}ms, p95: {:.2}ms, p99: {:.2}ms",
        slow_stats.avg_latency_ms,
        slow_stats.p50_latency_ms,
        slow_stats.p95_latency_ms,
        slow_stats.p99_latency_ms
    );

    info!("\n  Fast Provider:");
    info!(
        "    Avg: {:.2}ms, p50: {:.2}ms, p95: {:.2}ms, p99: {:.2}ms",
        fast_stats.avg_latency_ms,
        fast_stats.p50_latency_ms,
        fast_stats.p95_latency_ms,
        fast_stats.p99_latency_ms
    );

    // Verify latency metrics
    // Slow provider should have latency >= 100ms
    assert!(slow_stats.avg_latency_ms >= 100.0);
    assert!(slow_stats.p50_latency_ms >= 100.0);
    assert!(slow_stats.p95_latency_ms >= 100.0);

    // Fast provider should be much faster
    assert!(fast_stats.avg_latency_ms < 50.0);

    gateway.shutdown().await;
    info!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // SCENARIO 4: Provider Recovery
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 4: PROVIDER RECOVERY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  A provider starts failing, then recovers.");
    info!("  Metrics should reflect the transition over time.\n");

    // FirstN(10) means first 10 calls fail, then succeeds
    let recovering = Arc::new(
        RecordingProvider::new("recovering-provider").with_failure_mode(FailureMode::FirstN(10)),
    );

    let gateway = build_gateway(vec![Arc::clone(&recovering)]).await;

    // Send 5 requests during failure phase
    info!("  Phase 1: Failing (first 5 requests)");
    for i in 1..=5 {
        let _ = gateway
            .dispatch(make_action("recovering-provider"), None)
            .await;
        let stats = gateway
            .provider_metrics()
            .snapshot_for("recovering-provider")
            .unwrap();
        info!(
            "    Request {}: Success Rate: {:.1}%",
            i, stats.success_rate
        );
    }

    // Continue through failure boundary
    info!("\n  Phase 2: Transition (requests 6-10)");
    for i in 6..=10 {
        let _ = gateway
            .dispatch(make_action("recovering-provider"), None)
            .await;
        let stats = gateway
            .provider_metrics()
            .snapshot_for("recovering-provider")
            .unwrap();
        info!(
            "    Request {}: Success Rate: {:.1}%",
            i, stats.success_rate
        );
    }

    // Send requests during recovery phase
    info!("\n  Phase 3: Recovered (requests 11-20)");
    for i in 11..=20 {
        gateway
            .dispatch(make_action("recovering-provider"), None)
            .await?;
        let stats = gateway
            .provider_metrics()
            .snapshot_for("recovering-provider")
            .unwrap();
        info!(
            "    Request {}: Success Rate: {:.1}%",
            i, stats.success_rate
        );
    }

    let final_stats = gateway
        .provider_metrics()
        .snapshot_for("recovering-provider")
        .unwrap();

    info!("\n  Final Metrics:");
    info!(
        "    Total: {}, Successes: {}, Failures: {}",
        final_stats.total_requests, final_stats.successes, final_stats.failures
    );
    info!("    Success Rate: {:.1}%", final_stats.success_rate);

    // Verify recovery
    assert_eq!(final_stats.total_requests, 20);
    assert_eq!(final_stats.failures, 10); // First 10 failed
    assert_eq!(final_stats.successes, 10); // Last 10 succeeded
    assert!((final_stats.success_rate - 50.0).abs() < 1.0);

    gateway.shutdown().await;
    info!("\n  [Scenario 4 passed]\n");

    // =========================================================================
    // SCENARIO 5: Multiple Providers with Mixed Health
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 5: MULTIPLE PROVIDERS - MIXED HEALTH");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  3+ providers with different health characteristics.");
    info!("  Verify per-provider isolation and accurate metrics.\n");

    let healthy = Arc::new(RecordingProvider::new("healthy"));
    let slow = Arc::new(RecordingProvider::new("slow").with_delay(Duration::from_millis(50)));
    let unreliable =
        Arc::new(RecordingProvider::new("unreliable").with_failure_mode(FailureMode::EveryN(2)));
    let very_slow =
        Arc::new(RecordingProvider::new("very-slow").with_delay(Duration::from_millis(200)));

    let gateway = build_gateway(vec![
        Arc::clone(&healthy),
        Arc::clone(&slow),
        Arc::clone(&unreliable),
        Arc::clone(&very_slow),
    ])
    .await;

    // Send requests to all providers
    for _ in 0..50 {
        gateway.dispatch(make_action("healthy"), None).await?;
        gateway.dispatch(make_action("slow"), None).await?;
        let _ = gateway.dispatch(make_action("unreliable"), None).await;
        gateway.dispatch(make_action("very-slow"), None).await?;
    }

    // Display all metrics
    info!("  Provider Health Summary:");
    info!("  ┌─────────────┬──────────┬────────────┬──────────────┬──────────────┐");
    info!("  │ Provider    │ Requests │ Success %  │ Avg Lat (ms) │ p99 Lat (ms) │");
    info!("  ├─────────────┼──────────┼────────────┼──────────────┼──────────────┤");

    let all_stats = gateway.provider_metrics().snapshot();
    for name in ["healthy", "slow", "unreliable", "very-slow"] {
        let stats = all_stats.get(name).unwrap();
        info!(
            "  │ {:11} │ {:8} │ {:9.1}% │ {:12.2} │ {:12.2} │",
            name,
            stats.total_requests,
            stats.success_rate,
            stats.avg_latency_ms,
            stats.p99_latency_ms
        );
    }
    info!("  └─────────────┴──────────┴────────────┴──────────────┴──────────────┘");

    // Verify each provider
    let healthy_stats = all_stats.get("healthy").unwrap();
    assert_eq!(healthy_stats.total_requests, 50);
    assert_eq!(healthy_stats.successes, 50);
    assert!((healthy_stats.success_rate - 100.0).abs() < f64::EPSILON);

    let slow_stats = all_stats.get("slow").unwrap();
    assert!(slow_stats.avg_latency_ms >= 50.0);

    let unreliable_stats = all_stats.get("unreliable").unwrap();
    assert!(unreliable_stats.failures >= 20); // ~50% failure
    assert!(unreliable_stats.success_rate < 60.0);

    let very_slow_stats = all_stats.get("very-slow").unwrap();
    assert!(very_slow_stats.avg_latency_ms >= 200.0);

    gateway.shutdown().await;
    info!("\n  [Scenario 5 passed]\n");

    // =========================================================================
    // SCENARIO 6: Zero Requests Provider
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 6: ZERO REQUESTS PROVIDER");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  A provider is registered but never receives actions.");
    info!("  It should not appear in metrics (lazy initialization).\n");

    let active = Arc::new(RecordingProvider::new("active"));
    let _unused = Arc::new(RecordingProvider::new("unused"));

    let gateway = build_gateway(vec![Arc::clone(&active), _unused]).await;

    // Only send to active provider
    for _ in 0..5 {
        gateway.dispatch(make_action("active"), None).await?;
    }

    let all_stats = gateway.provider_metrics().snapshot();
    info!("  Providers with metrics: {}", all_stats.len());
    info!(
        "  Active provider requests: {}",
        all_stats.get("active").unwrap().total_requests
    );

    // Verify unused provider is not in metrics
    assert_eq!(all_stats.len(), 1);
    assert!(all_stats.contains_key("active"));
    assert!(!all_stats.contains_key("unused"));

    // Verify snapshot_for unused returns None
    assert!(gateway.provider_metrics().snapshot_for("unused").is_none());

    gateway.shutdown().await;
    info!("\n  [Scenario 6 passed]\n");

    // =========================================================================
    // SCENARIO 7: Circuit Breaker Interaction
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 7: CIRCUIT BREAKER INTERACTION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  A failing provider triggers circuit breaker.");
    info!("  Verify both circuit state and provider metrics are consistent.\n");

    let failing =
        Arc::new(RecordingProvider::new("failing").with_failure_mode(FailureMode::Always));

    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .executor_config(ExecutorConfig {
            max_retries: 0,
            ..ExecutorConfig::default()
        })
        .circuit_breaker(CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        })
        .provider(Arc::clone(&failing) as Arc<dyn acteon_provider::DynProvider>)
        .build()
        .expect("gateway should build");

    // Send requests to trip the circuit
    for i in 1..=5 {
        let outcome = gateway.dispatch(make_action("failing"), None).await;
        let stats = gateway.provider_metrics().snapshot_for("failing").unwrap();
        let cb_state = gateway
            .circuit_breakers()
            .unwrap()
            .get("failing")
            .unwrap()
            .state()
            .await;

        info!(
            "  Request {}: Outcome: {:?}, Circuit: {}, Provider Failures: {}",
            i,
            outcome
                .as_ref()
                .map(|o| format!("{:?}", o))
                .unwrap_or_else(|e| format!("Err: {}", e)),
            cb_state,
            stats.failures
        );
    }

    // Verify consistency
    let final_stats = gateway.provider_metrics().snapshot_for("failing").unwrap();
    let cb_state = gateway
        .circuit_breakers()
        .unwrap()
        .get("failing")
        .unwrap()
        .state()
        .await;

    info!("\n  Final State:");
    info!("    Circuit Breaker: {}", cb_state);
    info!("    Provider Stats:");
    info!("      Total Requests: {}", final_stats.total_requests);
    info!("      Failures: {}", final_stats.failures);
    info!("      Success Rate: {:.1}%", final_stats.success_rate);
    info!(
        "      Last Error: {:?}",
        final_stats.last_error.as_deref().unwrap_or("none")
    );

    // Verify circuit opened after 3 failures
    assert_eq!(cb_state.to_string(), "open");
    // Provider should only be called 3 times (circuit blocks the rest)
    assert_eq!(failing.call_count(), 3);
    assert_eq!(final_stats.total_requests, 3);
    assert_eq!(final_stats.failures, 3);
    assert!((final_stats.success_rate - 0.0).abs() < f64::EPSILON);
    assert!(final_stats.last_error.is_some());

    // Verify global metrics show circuit-open rejections
    let global_metrics = gateway.metrics().snapshot();
    assert_eq!(global_metrics.circuit_open, 2); // Requests 4-5

    gateway.shutdown().await;
    info!("\n  [Scenario 7 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║              ALL SCENARIOS PASSED                           ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
