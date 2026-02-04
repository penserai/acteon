//! Demonstration of Acteon with mixed backends for state and audit.
//!
//! This example shows realistic production configurations where you might want:
//! - Fast state storage (Redis/Memory) for low-latency dispatch
//! - Durable audit trail (PostgreSQL/ClickHouse) for compliance and analytics
//!
//! Prerequisites:
//!   # Redis for state (fast, ephemeral)
//!   docker run -d --name acteon-redis -p 6379:6379 redis:7-alpine
//!
//!   # PostgreSQL for audit (durable, queryable)
//!   docker run -d --name acteon-postgres -p 5433:5432 \
//!     -e POSTGRES_PASSWORD=postgres -e POSTGRES_USER=postgres \
//!     -e POSTGRES_DB=acteon_test postgres:16-alpine
//!
//!   # ClickHouse for audit (analytics-optimized)
//!   docker run -d --name acteon-clickhouse -p 8123:8123 \
//!     -e CLICKHOUSE_PASSWORD="" clickhouse/clickhouse-server:latest
//!
//! Run scenarios:
//!   # Redis state + PostgreSQL audit
//!   cargo run -p acteon-simulation --example mixed_backends_simulation \
//!     --features "redis,postgres" -- redis-postgres
//!
//!   # Redis state + ClickHouse audit
//!   cargo run -p acteon-simulation --example mixed_backends_simulation \
//!     --features "redis,clickhouse" -- redis-clickhouse
//!
//!   # Memory state + PostgreSQL audit (for testing)
//!   cargo run -p acteon-simulation --example mixed_backends_simulation \
//!     --features "postgres" -- memory-postgres
//!
//!   # Memory state + ClickHouse audit (for testing)
//!   cargo run -p acteon-simulation --example mixed_backends_simulation \
//!     --features "clickhouse" -- memory-clickhouse

use std::sync::Arc;

use acteon_audit::AuditQuery;
use acteon_audit::store::AuditStore;
use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::{FailingProvider, FailureMode, RecordingProvider};
use acteon_state::lock::DistributedLock;
use acteon_state::store::StateStore;

// Memory backends (always available)
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

// Optional Redis backends
#[cfg(feature = "redis")]
use acteon_state_redis::{RedisConfig, RedisDistributedLock, RedisStateStore};

// Optional PostgreSQL audit
#[cfg(feature = "postgres")]
use acteon_audit_postgres::{PostgresAuditConfig, PostgresAuditStore};

// Optional ClickHouse audit
#[cfg(feature = "clickhouse")]
use acteon_audit_clickhouse::{ClickHouseAuditConfig, ClickHouseAuditStore};

const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-notifications
    priority: 1
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 60
"#;

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

const THROTTLE_RULE: &str = r#"
rules:
  - name: throttle-alerts
    priority: 2
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: throttle
      max_count: 3
      window_seconds: 10
"#;

const REROUTE_RULES: &str = r#"
rules:
  # Reroute urgent notifications from email to SMS
  - name: reroute-urgent-to-sms
    priority: 1
    condition:
      all:
        - field: action.provider
          eq: "email"
        - field: action.payload.priority
          eq: "urgent"
    action:
      type: reroute
      target_provider: sms

  # Reroute high-value transactions to premium provider
  - name: reroute-high-value
    priority: 2
    condition:
      all:
        - field: action.action_type
          eq: "transaction"
        - field: action.payload.tier
          eq: "premium"
    action:
      type: reroute
      target_provider: premium

  # Reroute after-hours emails to Slack
  - name: reroute-after-hours
    priority: 3
    condition:
      all:
        - field: action.provider
          eq: "email"
        - field: action.payload.after_hours
          eq: true
    action:
      type: reroute
      target_provider: slack

  # Reroute by tenant tier
  - name: reroute-enterprise-tier
    priority: 4
    condition:
      field: action.metadata.tier
      eq: "enterprise"
    action:
      type: reroute
      target_provider: dedicated

  # Fallback: reroute retries to backup provider
  - name: reroute-retry-to-backup
    priority: 5
    condition:
      all:
        - field: action.provider
          eq: "email"
        - field: action.payload.is_retry
          eq: true
    action:
      type: reroute
      target_provider: email-backup
"#;

/// Available state backend configurations
enum StateBackend {
    Memory,
    #[cfg(feature = "redis")]
    Redis {
        url: String,
    },
}

/// Available audit backend configurations
enum AuditBackend {
    #[cfg(feature = "postgres")]
    Postgres { url: String },
    #[cfg(feature = "clickhouse")]
    ClickHouse { url: String },
}

struct MixedBackendConfig {
    name: String,
    state: StateBackend,
    audit: AuditBackend,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    let config = match scenario {
        #[cfg(all(feature = "redis", feature = "postgres"))]
        "redis-postgres" => MixedBackendConfig {
            name: "Redis State + PostgreSQL Audit".to_string(),
            state: StateBackend::Redis {
                url: std::env::var("REDIS_URL")
                    .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            },
            audit: AuditBackend::Postgres {
                url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                    "postgres://postgres:postgres@localhost:5433/acteon_test".to_string()
                }),
            },
        },

        #[cfg(all(feature = "redis", feature = "clickhouse"))]
        "redis-clickhouse" => MixedBackendConfig {
            name: "Redis State + ClickHouse Audit".to_string(),
            state: StateBackend::Redis {
                url: std::env::var("REDIS_URL")
                    .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            },
            audit: AuditBackend::ClickHouse {
                url: std::env::var("CLICKHOUSE_URL")
                    .unwrap_or_else(|_| "http://localhost:8123".to_string()),
            },
        },

        #[cfg(feature = "postgres")]
        "memory-postgres" => MixedBackendConfig {
            name: "Memory State + PostgreSQL Audit".to_string(),
            state: StateBackend::Memory,
            audit: AuditBackend::Postgres {
                url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                    "postgres://postgres:postgres@localhost:5433/acteon_test".to_string()
                }),
            },
        },

        #[cfg(feature = "clickhouse")]
        "memory-clickhouse" => MixedBackendConfig {
            name: "Memory State + ClickHouse Audit".to_string(),
            state: StateBackend::Memory,
            audit: AuditBackend::ClickHouse {
                url: std::env::var("CLICKHOUSE_URL")
                    .unwrap_or_else(|_| "http://localhost:8123".to_string()),
            },
        },

        _ => {
            println!("╔══════════════════════════════════════════════════════════════╗");
            println!("║         MIXED BACKENDS SIMULATION - USAGE                    ║");
            println!("╚══════════════════════════════════════════════════════════════╝\n");
            println!("Available scenarios (based on enabled features):\n");

            #[cfg(all(feature = "redis", feature = "postgres"))]
            println!("  redis-postgres    - Redis for state, PostgreSQL for audit");

            #[cfg(all(feature = "redis", feature = "clickhouse"))]
            println!("  redis-clickhouse  - Redis for state, ClickHouse for audit");

            #[cfg(feature = "postgres")]
            println!("  memory-postgres   - Memory for state, PostgreSQL for audit");

            #[cfg(feature = "clickhouse")]
            println!("  memory-clickhouse - Memory for state, ClickHouse for audit");

            println!("\nExample:");
            println!("  cargo run -p acteon-simulation --example mixed_backends_simulation \\");
            println!("    --features \"redis,postgres\" -- redis-postgres\n");

            println!("Environment variables:");
            println!("  REDIS_URL      - Redis connection URL (default: redis://localhost:6379)");
            println!("  DATABASE_URL   - PostgreSQL connection URL");
            println!("  CLICKHOUSE_URL - ClickHouse HTTP URL (default: http://localhost:8123)");

            return Ok(());
        }
    };

    run_simulation(config).await
}

async fn run_simulation(config: MixedBackendConfig) -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  ACTEON MIXED BACKENDS SIMULATION                            ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    println!("Configuration: {}\n", config.name);

    // Create state backend
    let (state, lock): (Arc<dyn StateStore>, Arc<dyn DistributedLock>) = match config.state {
        StateBackend::Memory => {
            println!("→ State backend: Memory (in-process)");
            let state = Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>;
            let lock = Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>;
            (state, lock)
        }
        #[cfg(feature = "redis")]
        StateBackend::Redis { url } => {
            println!("→ State backend: Redis at {}", url);
            let redis_config = RedisConfig {
                url: url.clone(),
                prefix: "acteon_mixed_".to_string(),
                pool_size: 10,
                connection_timeout: std::time::Duration::from_secs(5),
            };
            let state = Arc::new(RedisStateStore::new(&redis_config)?) as Arc<dyn StateStore>;
            let lock =
                Arc::new(RedisDistributedLock::new(&redis_config)?) as Arc<dyn DistributedLock>;
            (state, lock)
        }
    };
    println!("  ✓ State store connected");

    // Create audit backend
    let audit: Arc<dyn AuditStore> = match config.audit {
        #[cfg(feature = "postgres")]
        AuditBackend::Postgres { url } => {
            println!(
                "→ Audit backend: PostgreSQL at {}",
                url.split('@').last().unwrap_or(&url)
            );
            let audit_config = PostgresAuditConfig::new(&url).with_prefix("acteon_mixed_");
            Arc::new(PostgresAuditStore::new(&audit_config).await?)
        }
        #[cfg(feature = "clickhouse")]
        AuditBackend::ClickHouse { url } => {
            println!("→ Audit backend: ClickHouse at {}", url);
            let audit_config = ClickHouseAuditConfig::new(&url).with_prefix("acteon_mixed_");
            Arc::new(ClickHouseAuditStore::new(&audit_config).await?)
        }
    };
    println!("  ✓ Audit store connected\n");

    // Parse rules
    let frontend = YamlFrontend;
    let mut rules = frontend.parse(DEDUP_RULE)?;
    rules.extend(frontend.parse(SUPPRESSION_RULE)?);
    rules.extend(frontend.parse(THROTTLE_RULE)?);

    println!("✓ Loaded {} rules", rules.len());
    for rule in &rules {
        println!("  - {}: {:?}", rule.name, rule.action);
    }
    println!();

    // Create providers
    let email_provider = Arc::new(RecordingProvider::new("email"));
    let sms_provider = Arc::new(RecordingProvider::new("sms"));
    let push_provider = Arc::new(RecordingProvider::new("push"));

    // Build gateway
    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .audit(audit.clone())
        .audit_ttl_seconds(3600)
        .audit_store_payload(true)
        .rules(rules)
        .provider(email_provider.clone() as Arc<dyn DynProvider>)
        .provider(sms_provider.clone() as Arc<dyn DynProvider>)
        .provider(push_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    println!("✓ Gateway built with mixed backends\n");

    // =========================================================================
    // SCENARIO 1: Basic Execution with Cross-Backend Audit
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: BASIC EXECUTION (State + Audit Integration)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "welcome",
        serde_json::json!({
            "user_id": "user-001",
            "template": "welcome_email",
        }),
    );

    println!("→ Dispatching welcome email...");
    let outcome = gateway.dispatch(action.clone(), None).await?;
    println!("  Outcome: {:?}", outcome);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let audit_record = audit.get_by_action_id(&action.id.to_string()).await?;
    if let Some(record) = audit_record {
        println!("  ✓ Audit recorded in separate backend:");
        println!(
            "    Outcome: {}, Duration: {}ms",
            record.outcome, record.duration_ms
        );
    }

    // =========================================================================
    // SCENARIO 2: Deduplication with State, Audit in Different Store
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: DEDUPLICATION (State handles dedup, Audit records)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    // First notification
    let notify1 = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({
            "user_id": "user-002",
            "message": "Order shipped",
        }),
    )
    .with_dedup_key("order-shipped-user-002");

    println!("→ Dispatching FIRST notification...");
    let outcome1 = gateway.dispatch(notify1.clone(), None).await?;
    println!("  Outcome: {:?}", outcome1);

    // Duplicate notification
    let notify2 = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({
            "user_id": "user-002",
            "message": "Order shipped (retry)",
        }),
    )
    .with_dedup_key("order-shipped-user-002");

    println!("→ Dispatching DUPLICATE notification...");
    let outcome2 = gateway.dispatch(notify2.clone(), None).await?;
    println!("  Outcome: {:?}", outcome2);

    println!(
        "  Email provider calls: {} (should be 1)",
        email_provider.call_count()
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify both are audited
    let record1 = audit.get_by_action_id(&notify1.id.to_string()).await?;
    let record2 = audit.get_by_action_id(&notify2.id.to_string()).await?;

    println!("\n  Audit trail verification:");
    if let Some(r) = record1 {
        println!(
            "    First action:  {} ({})",
            r.outcome,
            r.action_id[..8].to_string()
        );
    }
    if let Some(r) = record2 {
        println!(
            "    Second action: {} ({})",
            r.outcome,
            r.action_id[..8].to_string()
        );
    }

    // =========================================================================
    // SCENARIO 3: Throttling with Audit Trail
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: THROTTLING (Rate limit in state, all audited)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    push_provider.clear();

    println!("→ Dispatching 5 alert actions (throttle limit: 3/10s)...\n");

    let mut alert_outcomes = vec![];
    for i in 1..=5 {
        let alert = Action::new(
            "alerts",
            "tenant-1",
            "push",
            "alert",
            serde_json::json!({
                "severity": "warning",
                "message": format!("Alert #{}", i),
            }),
        );

        let outcome = gateway.dispatch(alert, None).await?;
        alert_outcomes.push(outcome.clone());
        println!("  Alert #{}: {:?}", i, outcome);
    }

    let executed = alert_outcomes
        .iter()
        .filter(|o| matches!(o, acteon_core::ActionOutcome::Executed(_)))
        .count();
    let throttled = alert_outcomes
        .iter()
        .filter(|o| matches!(o, acteon_core::ActionOutcome::Throttled { .. }))
        .count();

    println!(
        "\n  Results: {} executed, {} throttled",
        executed, throttled
    );
    println!(
        "  Push provider calls: {} (should be 3)",
        push_provider.call_count()
    );

    // =========================================================================
    // SCENARIO 4: Multi-Provider Dispatch
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: MULTI-PROVIDER (Different channels, unified audit)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();
    sms_provider.clear();
    push_provider.clear();

    // Send via different providers
    let providers = ["email", "sms", "push"];
    for provider in providers {
        let action = Action::new(
            "marketing",
            "tenant-2",
            provider,
            "campaign",
            serde_json::json!({
                "campaign_id": "summer-sale-2024",
                "channel": provider,
            }),
        );

        let outcome = gateway.dispatch(action, None).await?;
        println!("  {} campaign: {:?}", provider.to_uppercase(), outcome);
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Query unified audit by tenant
    let tenant2_query = AuditQuery {
        tenant: Some("tenant-2".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let tenant2_page = audit.query(&tenant2_query).await?;
    println!(
        "\n  Unified audit for tenant-2: {} records",
        tenant2_page.total
    );
    for record in &tenant2_page.records {
        println!(
            "    - {} via {} ({})",
            record.action_type, record.provider, record.outcome
        );
    }

    // =========================================================================
    // SCENARIO 5: Concurrent Dispatch with Mixed Backends
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 5: CONCURRENT DISPATCH (Stress test mixed backends)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    let gateway_arc = Arc::new(gateway);
    let mut handles = vec![];

    println!("→ Dispatching 20 concurrent actions with same dedup key...");

    for i in 0..20 {
        let gw = Arc::clone(&gateway_arc);
        let handle = tokio::spawn(async move {
            let action = Action::new(
                "stress",
                "tenant-3",
                "email",
                "notify",
                serde_json::json!({
                    "worker": i,
                }),
            )
            .with_dedup_key("concurrent-stress-test");

            gw.dispatch(action, None).await
        });
        handles.push(handle);
    }

    let mut executed = 0;
    let mut deduplicated = 0;

    for handle in handles {
        match handle.await? {
            Ok(acteon_core::ActionOutcome::Executed(_)) => executed += 1,
            Ok(acteon_core::ActionOutcome::Deduplicated) => deduplicated += 1,
            Ok(other) => println!("  Unexpected: {:?}", other),
            Err(e) => println!("  Error: {}", e),
        }
    }

    println!("  Executed: {}, Deduplicated: {}", executed, deduplicated);
    println!("  Email provider calls: {}", email_provider.call_count());

    // =========================================================================
    // SCENARIO 6: Throughput Benchmark
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 6: THROUGHPUT BENCHMARK");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    let batch_size = 200;
    let actions: Vec<Action> = (0..batch_size)
        .map(|i| {
            Action::new(
                "benchmark",
                "tenant-bench",
                "email",
                "bulk",
                serde_json::json!({"seq": i}),
            )
        })
        .collect();

    println!("→ Dispatching {} actions sequentially...", batch_size);
    let start = std::time::Instant::now();

    for action in actions {
        gateway_arc.dispatch(action, None).await?;
    }

    let elapsed = start.elapsed();
    let throughput = batch_size as f64 / elapsed.as_secs_f64();

    println!("  Completed in: {:?}", elapsed);
    println!("  Throughput: {:.0} actions/sec", throughput);

    // Wait for audit writes
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Verify all audited
    let bench_query = AuditQuery {
        namespace: Some("benchmark".to_string()),
        limit: Some(1),
        ..Default::default()
    };
    let bench_page = audit.query(&bench_query).await?;
    println!("  Audit records created: {}", bench_page.total);

    // =========================================================================
    // SCENARIO 7: Failure Injection
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 7: FAILURE INJECTION (Provider errors, retries)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // 7a: FailingProvider - always fails
    println!("  7a) FailingProvider (always fails):");
    let failing_webhook = Arc::new(FailingProvider::connection_error(
        "webhook",
        "Connection refused: service unavailable",
    ));

    let gw_with_failing = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .audit(audit.clone())
        .provider(failing_webhook.clone() as Arc<dyn DynProvider>)
        .build()?;

    let webhook_action = Action::new(
        "webhooks",
        "tenant-fail",
        "webhook",
        "notify",
        serde_json::json!({
            "url": "https://api.example.com/webhook",
            "event": "user.created",
        }),
    );

    println!("      → Dispatching to failing webhook provider...");
    let result = gw_with_failing.dispatch(webhook_action.clone(), None).await;
    match &result {
        Ok(outcome) => println!("      Outcome: {:?}", outcome),
        Err(e) => println!("      Error (expected): {}", e),
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check audit - failures should still be recorded
    let fail_audit = audit
        .get_by_action_id(&webhook_action.id.to_string())
        .await?;
    if let Some(record) = fail_audit {
        println!("      ✓ Failure audited: outcome={}", record.outcome);
    }

    gw_with_failing.shutdown().await;

    // 7b: FailingProvider - fail then recover
    println!("\n  7b) FailingProvider with recovery (fail first 2, then succeed):");
    let recovering_provider = Arc::new(
        FailingProvider::execution_failed("recovering", "Temporary failure").fail_until(2),
    );

    let gw_recovering = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .audit(audit.clone())
        .provider(recovering_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    for i in 1..=4 {
        let action = Action::new(
            "recovery",
            "tenant-recover",
            "recovering",
            "test",
            serde_json::json!({
                "attempt": i,
            }),
        );

        let result = gw_recovering.dispatch(action, None).await;
        match result {
            Ok(acteon_core::ActionOutcome::Executed(_)) => {
                println!("      Attempt #{}: SUCCESS", i)
            }
            Ok(outcome) => println!("      Attempt #{}: {:?}", i, outcome),
            Err(e) => println!("      Attempt #{}: FAILED - {}", i, e),
        }
    }
    println!(
        "      Provider call count: {}",
        recovering_provider.call_count()
    );

    gw_recovering.shutdown().await;

    // 7c: RecordingProvider with FailureMode
    println!("\n  7c) RecordingProvider with FailureMode::EveryN(3):");
    let flaky_provider =
        Arc::new(RecordingProvider::new("flaky").with_failure_mode(FailureMode::EveryN(3)));

    let gw_flaky = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .audit(audit.clone())
        .provider(flaky_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    println!("      → Dispatching 6 actions (every 3rd fails):");
    for i in 1..=6 {
        let action = Action::new(
            "flaky",
            "tenant-flaky",
            "flaky",
            "test",
            serde_json::json!({
                "seq": i,
            }),
        );

        let result = gw_flaky.dispatch(action, None).await;
        let status = match result {
            Ok(acteon_core::ActionOutcome::Executed(_)) => "OK",
            Err(_) => "FAIL",
            _ => "OTHER",
        };
        print!("      #{}: {}  ", i, status);
    }
    println!(
        "\n      Provider calls: {}, Successful: {}",
        flaky_provider.call_count(),
        flaky_provider
            .calls()
            .iter()
            .filter(|c| c.response.is_ok())
            .count()
    );

    gw_flaky.shutdown().await;

    // 7d: RecordingProvider with simulated latency
    println!("\n  7d) RecordingProvider with simulated latency (50ms):");
    let slow_provider =
        Arc::new(RecordingProvider::new("slow").with_delay(std::time::Duration::from_millis(50)));

    let gw_slow = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .provider(slow_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    println!("      → Dispatching 5 actions with latency...");
    let start = std::time::Instant::now();
    for i in 1..=5 {
        let action = Action::new(
            "slow",
            "tenant-slow",
            "slow",
            "test",
            serde_json::json!({"i": i}),
        );
        gw_slow.dispatch(action, None).await?;
    }
    let elapsed = start.elapsed();
    println!("      Total time: {:?} (expected ~250ms)", elapsed);
    println!("      Avg latency: {:?}/action", elapsed / 5);

    gw_slow.shutdown().await;

    // 7e: RecordingProvider with custom response
    println!("\n  7e) RecordingProvider with custom response function:");
    let custom_provider = Arc::new(RecordingProvider::new("custom").with_response_fn(|action| {
        // Simulate different responses based on action payload
        let priority = action
            .payload
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("normal");

        Ok(acteon_core::ProviderResponse {
            status: acteon_core::ResponseStatus::Success,
            body: serde_json::json!({
                "processed": true,
                "priority": priority,
                "queue": if priority == "high" { "fast-lane" } else { "standard" },
                "eta_seconds": if priority == "high" { 1 } else { 30 },
            }),
            headers: std::collections::HashMap::new(),
        })
    }));

    let gw_custom = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .provider(custom_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    for priority in ["normal", "high"] {
        let action = Action::new(
            "custom",
            "tenant-custom",
            "custom",
            "process",
            serde_json::json!({
                "priority": priority,
                "data": "test",
            }),
        );

        if let Ok(acteon_core::ActionOutcome::Executed(resp)) =
            gw_custom.dispatch(action, None).await
        {
            println!(
                "      priority={}: queue={}, eta={}s",
                priority,
                resp.body.get("queue").unwrap(),
                resp.body.get("eta_seconds").unwrap()
            );
        }
    }

    gw_custom.shutdown().await;

    // =========================================================================
    // SCENARIO 8: Rerouting (If-Then-Else Logic)
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 8: REROUTING (If-Then-Else Logic)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Create providers for all reroute targets
    let email_reroute = Arc::new(RecordingProvider::new("email"));
    let sms_reroute = Arc::new(RecordingProvider::new("sms"));
    let slack_reroute = Arc::new(RecordingProvider::new("slack"));
    let premium_reroute = Arc::new(RecordingProvider::new("premium"));
    let dedicated_reroute = Arc::new(RecordingProvider::new("dedicated"));
    let email_backup_reroute = Arc::new(RecordingProvider::new("email-backup"));

    // Parse reroute rules
    let reroute_rules = YamlFrontend.parse(REROUTE_RULES)?;

    let gw_reroute = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .audit(audit.clone())
        .rules(reroute_rules)
        .provider(email_reroute.clone() as Arc<dyn DynProvider>)
        .provider(sms_reroute.clone() as Arc<dyn DynProvider>)
        .provider(slack_reroute.clone() as Arc<dyn DynProvider>)
        .provider(premium_reroute.clone() as Arc<dyn DynProvider>)
        .provider(dedicated_reroute.clone() as Arc<dyn DynProvider>)
        .provider(email_backup_reroute.clone() as Arc<dyn DynProvider>)
        .build()?;

    println!("  Rules loaded:");
    println!("    - urgent email → SMS");
    println!("    - premium-tier transaction → premium provider");
    println!("    - after-hours email → Slack");
    println!("    - enterprise tenant → dedicated provider");
    println!("    - retry email → email-backup\n");

    // 8a: Normal email (no reroute)
    println!("  8a) Normal email (no reroute condition):");
    let normal_email = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Hello",
            "priority": "normal",
        }),
    );
    let outcome = gw_reroute.dispatch(normal_email, None).await?;
    println!("      → Original provider: email");
    println!("      → Executed by: email (no reroute)");
    println!(
        "      Email calls: {}, SMS calls: {}",
        email_reroute.call_count(),
        sms_reroute.call_count()
    );

    // 8b: Urgent email → SMS
    println!("\n  8b) Urgent email → rerouted to SMS:");
    email_reroute.clear();
    sms_reroute.clear();

    let urgent_email = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "URGENT: Server down!",
            "priority": "urgent",
        }),
    );
    let outcome = gw_reroute.dispatch(urgent_email.clone(), None).await?;
    println!("      → Original provider: email");
    println!("      → Rerouted to: sms (because priority=urgent)");
    println!(
        "      Email calls: {}, SMS calls: {}",
        email_reroute.call_count(),
        sms_reroute.call_count()
    );

    // Verify audit shows reroute
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    if let Some(record) = audit.get_by_action_id(&urgent_email.id.to_string()).await? {
        println!(
            "      Audit: verdict={}, provider={}",
            record.verdict, record.provider
        );
    }

    // 8c: Premium-tier transaction → premium provider
    println!("\n  8c) Premium-tier transaction → premium provider:");
    premium_reroute.clear();

    let high_value = Action::new(
        "payments",
        "tenant-1",
        "standard",
        "transaction",
        serde_json::json!({
            "amount": 5000,
            "currency": "USD",
            "tier": "premium",
        }),
    );
    let outcome = gw_reroute.dispatch(high_value, None).await?;
    println!("      → Original provider: standard");
    println!("      → Rerouted to: premium (because payload.tier=premium)");
    println!("      Premium calls: {}", premium_reroute.call_count());

    // 8d: Standard-tier transaction (no reroute)
    println!("\n  8d) Standard-tier transaction → no reroute:");
    premium_reroute.clear();

    let low_value = Action::new(
        "payments",
        "tenant-1",
        "standard",
        "transaction",
        serde_json::json!({
            "amount": 50,
            "currency": "USD",
            "tier": "standard",
        }),
    );
    // This will fail because we don't have a "standard" provider, but that's ok for demo
    let _ = gw_reroute.dispatch(low_value, None).await;
    println!("      → Original provider: standard (no reroute, tier != premium)");
    println!(
        "      Premium calls: {} (should be 0)",
        premium_reroute.call_count()
    );

    // 8e: After-hours email → Slack
    println!("\n  8e) After-hours email → rerouted to Slack:");
    email_reroute.clear();
    slack_reroute.clear();

    let after_hours = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send",
        serde_json::json!({
            "to": "oncall@example.com",
            "subject": "System alert",
            "after_hours": true,
        }),
    );
    let outcome = gw_reroute.dispatch(after_hours, None).await?;
    println!("      → Original provider: email");
    println!("      → Rerouted to: slack (because after_hours=true)");
    println!(
        "      Email calls: {}, Slack calls: {}",
        email_reroute.call_count(),
        slack_reroute.call_count()
    );

    // 8f: Enterprise tier → dedicated
    println!("\n  8f) Enterprise tenant → dedicated provider:");
    dedicated_reroute.clear();

    let enterprise = Action::new(
        "notifications",
        "enterprise-corp",
        "email",
        "send",
        serde_json::json!({
            "to": "ceo@enterprise.com",
            "subject": "Report",
        }),
    )
    .with_metadata(acteon_core::ActionMetadata {
        labels: [("tier".to_string(), "enterprise".to_string())]
            .into_iter()
            .collect(),
    });
    let outcome = gw_reroute.dispatch(enterprise, None).await?;
    println!("      → Original provider: email");
    println!("      → Rerouted to: dedicated (because metadata.tier=enterprise)");
    println!("      Dedicated calls: {}", dedicated_reroute.call_count());

    // 8g: Retry fallback → email-backup
    println!("\n  8g) Retry email → backup provider:");
    email_reroute.clear();
    email_backup_reroute.clear();

    let retry_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Retrying...",
            "is_retry": true,
        }),
    );
    let outcome = gw_reroute.dispatch(retry_action, None).await?;
    println!("      → Original provider: email");
    println!("      → Rerouted to: email-backup (because is_retry=true)");
    println!(
        "      Email calls: {}, Backup calls: {}",
        email_reroute.call_count(),
        email_backup_reroute.call_count()
    );

    // 8h: Multiple conditions - urgent + after_hours
    println!("\n  8h) Multiple matching rules (priority wins):");
    email_reroute.clear();
    sms_reroute.clear();
    slack_reroute.clear();

    let multi_match = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send",
        serde_json::json!({
            "to": "oncall@example.com",
            "subject": "CRITICAL",
            "priority": "urgent",
            "after_hours": true,
        }),
    );
    let outcome = gw_reroute.dispatch(multi_match, None).await?;
    println!(
        "      → Matches: reroute-urgent-to-sms (priority=1), reroute-after-hours (priority=3)"
    );
    println!("      → Winner: SMS (higher priority rule)");
    println!(
        "      SMS calls: {}, Slack calls: {}",
        sms_reroute.call_count(),
        slack_reroute.call_count()
    );

    // Summary
    println!("\n  Rerouting Summary:");
    println!(
        "    Total email provider calls: {}",
        email_reroute.call_count()
    );
    println!("    Total SMS provider calls: {}", sms_reroute.call_count());
    println!(
        "    Total Slack provider calls: {}",
        slack_reroute.call_count()
    );
    println!(
        "    Total Premium provider calls: {}",
        premium_reroute.call_count()
    );
    println!(
        "    Total Dedicated provider calls: {}",
        dedicated_reroute.call_count()
    );
    println!(
        "    Total Backup provider calls: {}",
        email_backup_reroute.call_count()
    );

    gw_reroute.shutdown().await;

    // =========================================================================
    // Summary
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  AUDIT TRAIL SUMMARY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let all_query = AuditQuery {
        limit: Some(1000),
        ..Default::default()
    };
    let all_page = audit.query(&all_query).await?;

    let executed_count = all_page
        .records
        .iter()
        .filter(|r| r.outcome == "executed")
        .count();
    let suppressed_count = all_page
        .records
        .iter()
        .filter(|r| r.outcome == "suppressed")
        .count();
    let deduplicated_count = all_page
        .records
        .iter()
        .filter(|r| r.outcome == "deduplicated")
        .count();
    let throttled_count = all_page
        .records
        .iter()
        .filter(|r| r.outcome == "throttled")
        .count();

    println!("  Total audit records: {}", all_page.total);
    println!("    - Executed: {}", executed_count);
    println!("    - Suppressed: {}", suppressed_count);
    println!("    - Deduplicated: {}", deduplicated_count);
    println!("    - Throttled: {}", throttled_count);

    // Unique tenants
    let tenants: std::collections::HashSet<_> =
        all_page.records.iter().map(|r| &r.tenant).collect();
    println!("\n  Tenants tracked: {:?}", tenants);

    // Unique providers
    let providers: std::collections::HashSet<_> =
        all_page.records.iter().map(|r| &r.provider).collect();
    println!("  Providers tracked: {:?}", providers);

    gateway_arc.shutdown().await;
    println!("\n✓ Gateway shut down gracefully\n");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           MIXED BACKENDS SIMULATION COMPLETE                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
