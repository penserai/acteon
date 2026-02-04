# acteon-simulation

Simulation and testing framework for Acteon. This crate provides tools for end-to-end testing of the Acteon gateway against real or mock backends, with full audit trail verification and failure injection capabilities.

## Features

- **Mock Providers**: `RecordingProvider` captures all calls for verification; `FailingProvider` simulates errors
- **Mixed Backend Testing**: Combine any state backend (Memory, Redis, PostgreSQL, DynamoDB, ClickHouse) with any audit backend
- **Failure Injection**: Simulate timeouts, connection errors, rate limiting, and transient failures
- **End-to-End Audit Trail**: Verify that all action outcomes (executed, suppressed, deduplicated, throttled, failed) are recorded
- **Performance Benchmarks**: Measure throughput across different backend combinations

## Installation

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
acteon-simulation = { path = "../acteon-simulation" }

# Enable specific backends
acteon-simulation = { path = "../acteon-simulation", features = ["redis", "postgres"] }
```

### Feature Flags

| Feature | Description |
|---------|-------------|
| `redis` | Enable Redis state backend |
| `postgres` | Enable PostgreSQL state + audit backends |
| `dynamodb` | Enable DynamoDB state backend |
| `clickhouse` | Enable ClickHouse state + audit backends |

## Core Components

### RecordingProvider

A mock provider that captures all execution calls for later verification.

```rust
use acteon_simulation::{RecordingProvider, FailureMode};
use acteon_gateway::GatewayBuilder;
use std::sync::Arc;

// Basic usage
let email_provider = Arc::new(RecordingProvider::new("email"));

// With simulated latency
let slow_provider = Arc::new(
    RecordingProvider::new("slow-api")
        .with_delay(Duration::from_millis(100))
);

// With failure injection
let flaky_provider = Arc::new(
    RecordingProvider::new("flaky")
        .with_failure_mode(FailureMode::EveryN(5))  // Every 5th call fails
);

// With custom response logic
let smart_provider = Arc::new(
    RecordingProvider::new("smart")
        .with_response_fn(|action| {
            let priority = action.payload.get("priority").and_then(|v| v.as_str());
            Ok(ProviderResponse::success(json!({
                "queue": if priority == Some("high") { "fast" } else { "standard" }
            })))
        })
);

// Register with gateway
let gateway = GatewayBuilder::new()
    .provider(email_provider.clone() as Arc<dyn DynProvider>)
    .build()?;

// After dispatching actions, verify calls
email_provider.assert_called(1);
email_provider.assert_not_called();
email_provider.assert_called_at_least(5);

// Inspect captured calls
for call in email_provider.calls() {
    println!("Action: {}, Response: {:?}", call.action.id, call.response);
}

// Get last action
let last = email_provider.last_action().unwrap();

// Reset between tests
email_provider.clear();
```

#### FailureMode Options

| Mode | Description |
|------|-------------|
| `FailureMode::None` | Never fail (default) |
| `FailureMode::Always` | Always fail |
| `FailureMode::FirstN(n)` | Fail the first N calls, then succeed |
| `FailureMode::EveryN(n)` | Fail every Nth call |
| `FailureMode::Probabilistic(p)` | Fail with probability p (0.0 to 1.0) |

### FailingProvider

A provider that simulates specific failure scenarios.

```rust
use acteon_simulation::{FailingProvider, FailureType};
use std::time::Duration;

// Always fails with connection error (retryable)
let failing = FailingProvider::connection_error("webhook", "Connection refused");

// Always fails with timeout (retryable)
let timeout = FailingProvider::timeout("slow-api", Duration::from_secs(30));

// Always fails with rate limiting (retryable)
let rate_limited = FailingProvider::rate_limited("api");

// Always fails with execution error (non-retryable)
let broken = FailingProvider::execution_failed("broken", "Internal error");

// Transient failure: fail first N calls, then recover
let recovering = FailingProvider::execution_failed("flaky", "Temporary failure")
    .fail_until(3);  // Calls 1-3 fail, call 4+ succeed

// Check call count
println!("Attempts: {}", recovering.call_count());
recovering.reset();
```

#### FailureType Options

| Type | Retryable | Use Case |
|------|-----------|----------|
| `ExecutionFailed(msg)` | No | Permanent provider errors |
| `Timeout(duration)` | Yes | Slow/unresponsive services |
| `Connection(msg)` | Yes | Network failures |
| `RateLimited` | Yes | API rate limits |
| `Configuration(msg)` | No | Invalid setup |

### SimulationHarness

High-level test orchestrator for multi-node scenarios.

```rust
use acteon_simulation::prelude::*;
use acteon_core::Action;

#[tokio::test]
async fn test_deduplication() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(r#"
                rules:
                  - name: dedup-emails
                    condition:
                      field: action.action_type
                      eq: "send_email"
                    action:
                      type: deduplicate
                      ttl_seconds: 60
            "#)
            .build()
    ).await.unwrap();

    // First action executes
    let action1 = Action::new("ns", "tenant", "email", "send_email", json!({}))
        .with_dedup_key("unique-key");
    let outcome1 = harness.dispatch(&action1).await.unwrap();
    outcome1.assert_executed();

    // Second action with same key is deduplicated
    let action2 = Action::new("ns", "tenant", "email", "send_email", json!({}))
        .with_dedup_key("unique-key");
    let outcome2 = harness.dispatch(&action2).await.unwrap();
    outcome2.assert_deduplicated();

    // Provider called only once
    harness.provider("email").unwrap().assert_called(1);

    harness.teardown().await.unwrap();
}
```

## Running Simulations

### Prerequisites

Start the required Docker containers:

```sh
# Redis
docker run -d --name acteon-redis -p 6379:6379 redis:7-alpine

# PostgreSQL
docker run -d --name acteon-postgres -p 5433:5432 \
  -e POSTGRES_PASSWORD=postgres -e POSTGRES_USER=postgres \
  -e POSTGRES_DB=acteon_test postgres:16-alpine

# ClickHouse
docker run -d --name acteon-clickhouse -p 8123:8123 -p 9000:9000 \
  -e CLICKHOUSE_PASSWORD="" clickhouse/clickhouse-server:latest

# DynamoDB Local
docker run -d --name acteon-dynamodb -p 8000:8000 amazon/dynamodb-local:latest
```

### Available Simulations

#### Single Backend Simulations

```sh
# Redis state backend
cargo run -p acteon-simulation --example redis_simulation --features redis

# PostgreSQL state + audit
cargo run -p acteon-simulation --example postgres_simulation --features postgres

# ClickHouse state + audit
cargo run -p acteon-simulation --example clickhouse_simulation --features clickhouse

# DynamoDB state backend
cargo run -p acteon-simulation --example dynamodb_simulation --features dynamodb
```

#### Mixed Backend Simulations

```sh
# Redis state + PostgreSQL audit (production-ready combo)
cargo run -p acteon-simulation --example mixed_backends_simulation \
  --features "redis,postgres" -- redis-postgres

# Redis state + ClickHouse audit (analytics-optimized)
cargo run -p acteon-simulation --example mixed_backends_simulation \
  --features "redis,clickhouse" -- redis-clickhouse

# Memory state + PostgreSQL audit (fast testing)
cargo run -p acteon-simulation --example mixed_backends_simulation \
  --features "postgres" -- memory-postgres

# Memory state + ClickHouse audit
cargo run -p acteon-simulation --example mixed_backends_simulation \
  --features "clickhouse" -- memory-clickhouse
```

## Test Scenarios

The simulations cover the following scenarios:

### 1. Basic Execution with Audit Trail

Dispatches an action and verifies:
- Provider receives the action
- Audit record is created with correct metadata
- Payload is captured (if `audit_store_payload` enabled)

### 2. Deduplication

Tests the deduplicate rule action:
- First action with a dedup_key executes
- Second action with same dedup_key is deduplicated
- Provider is called only once
- Both actions are audited (one as `executed`, one as `deduplicated`)

### 3. Suppression

Tests the suppress rule action:
- Actions matching suppression rules are blocked
- Provider is never called
- Action is audited with `outcome=suppressed` and `matched_rule` recorded

### 4. Throttling

Tests the throttle rule action:
- Actions up to `max_count` within `window_seconds` execute
- Excess actions are throttled with `retry_after` hint
- All actions (executed and throttled) are audited

### 5. Multi-Provider Dispatch

Dispatches to different providers (email, sms, push) and verifies:
- Each provider receives correct actions
- Unified audit trail tracks all providers
- Can query audit by provider, tenant, or action_type

### 6. Concurrent Dispatch

Stress tests distributed locking:
- Dispatches N concurrent actions with same dedup_key
- Verifies only 1 executes (others deduplicated)
- Measures lock contention behavior per backend

### 7. Failure Injection

Tests error handling and audit trail for failures:
- `FailingProvider` that always fails
- `FailingProvider` with recovery after N failures
- `RecordingProvider` with `FailureMode::EveryN`
- Simulated latency with `with_delay()`
- Custom response functions

### 8. Throughput Benchmark

Measures actions/second for each backend combination.

## Performance Findings

Throughput measured with 200 sequential actions (no rules, single provider):

| State Backend | Audit Backend | Throughput | Notes |
|---------------|---------------|------------|-------|
| Memory | None | ~50,000/sec | Baseline, no I/O |
| Memory | PostgreSQL | ~28,000/sec | Async audit writes |
| Redis | None | ~2,000/sec | Network round-trip per action |
| Redis | PostgreSQL | ~1,100/sec | Production-ready combo |
| Redis | ClickHouse | ~970/sec | Analytics-optimized |
| PostgreSQL | PostgreSQL | ~850/sec | Single backend for both |
| DynamoDB | None | ~340/sec | AWS SDK overhead |
| ClickHouse | ClickHouse | ~120/sec | Eventual consistency model |

### Concurrent Deduplication Accuracy

With 20 concurrent dispatches using the same dedup_key:

| State Backend | Executed | Deduplicated | Notes |
|---------------|----------|--------------|-------|
| Memory | 1 | 19 | Perfect (single process) |
| Redis | 1 | 19 | Strong locking |
| PostgreSQL | 1 | 19 | ACID guarantees |
| DynamoDB | 1 | 19 | Conditional writes |
| ClickHouse | 10-20 | 0-10 | Eventual consistency* |

*ClickHouse uses ReplacingMergeTree which provides eventual consistency. Not recommended for strict deduplication requirements.

## Audit Trail Verification

All simulations verify end-to-end audit trail recording:

```rust
// Query audit by action ID
let record = audit.get_by_action_id(&action.id.to_string()).await?;
assert_eq!(record.outcome, "executed");
assert_eq!(record.provider, "email");

// Query with filters
let page = audit.query(&AuditQuery {
    tenant: Some("tenant-1".to_string()),
    outcome: Some("suppressed".to_string()),
    limit: Some(100),
    ..Default::default()
}).await?;

// Verify all outcome types are tracked
let outcomes: HashSet<_> = page.records.iter().map(|r| &r.outcome).collect();
assert!(outcomes.contains(&"executed".to_string()));
assert!(outcomes.contains(&"suppressed".to_string()));
assert!(outcomes.contains(&"deduplicated".to_string()));
assert!(outcomes.contains(&"throttled".to_string()));
assert!(outcomes.contains(&"failed".to_string()));
```

## Example: Full Integration Test

```rust
use acteon_simulation::prelude::*;
use acteon_core::Action;

const RULES: &str = r#"
rules:
  - name: dedup-notifications
    priority: 1
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 60

  - name: block-spam
    priority: 2
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress

  - name: throttle-alerts
    priority: 3
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: throttle
      max_count: 10
      window_seconds: 60
"#;

#[tokio::test]
async fn full_pipeline_test() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(RULES)
            .build()
    ).await.unwrap();

    // Test execution
    let action = Action::new("ns", "t1", "email", "welcome", json!({}));
    harness.dispatch(&action).await.unwrap().assert_executed();
    harness.provider("email").unwrap().assert_called(1);

    // Test deduplication
    let notify1 = Action::new("ns", "t1", "email", "notify", json!({}))
        .with_dedup_key("dup-key");
    let notify2 = Action::new("ns", "t1", "email", "notify", json!({}))
        .with_dedup_key("dup-key");

    harness.dispatch(&notify1).await.unwrap().assert_executed();
    harness.dispatch(&notify2).await.unwrap().assert_deduplicated();

    // Test suppression
    let spam = Action::new("ns", "t1", "email", "spam", json!({}));
    harness.dispatch(&spam).await.unwrap().assert_suppressed();

    // Test throttling
    for i in 0..15 {
        let alert = Action::new("ns", "t1", "sms", "alert", json!({"seq": i}));
        let outcome = harness.dispatch(&alert).await.unwrap();
        if i < 10 {
            outcome.assert_executed();
        } else {
            outcome.assert_throttled();
        }
    }

    // Verify provider call counts
    harness.provider("email").unwrap().assert_called(2);  // welcome + notify1
    harness.provider("sms").unwrap().assert_called(10);   // first 10 alerts

    harness.teardown().await.unwrap();
}
```

## Benchmarks

Run the included benchmarks:

```sh
# Throughput benchmark
cargo bench -p acteon-simulation --bench throughput

# Latency benchmark
cargo bench -p acteon-simulation --bench latency
```

## License

Copyright 2026 Penserai Inc. Licensed under Apache-2.0.
