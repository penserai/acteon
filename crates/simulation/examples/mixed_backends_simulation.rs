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
use tracing::info;

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
    tracing_subscriber::fmt::init();

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
            info!("╔══════════════════════════════════════════════════════════════╗");
            info!("║         MIXED BACKENDS SIMULATION - USAGE                    ║");
            info!("╚══════════════════════════════════════════════════════════════╝\n");
            info!("Available scenarios (based on enabled features):\n");

            #[cfg(all(feature = "redis", feature = "postgres"))]
            info!("  redis-postgres    - Redis for state, PostgreSQL for audit");

            #[cfg(all(feature = "redis", feature = "clickhouse"))]
            info!("  redis-clickhouse  - Redis for state, ClickHouse for audit");

            #[cfg(feature = "postgres")]
            info!("  memory-postgres   - Memory for state, PostgreSQL for audit");

            #[cfg(feature = "clickhouse")]
            info!("  memory-clickhouse - Memory for state, ClickHouse for audit");

            info!("\nExample:");
            info!("  cargo run -p acteon-simulation --example mixed_backends_simulation \\");
            info!("    --features \"redis,postgres\" -- redis-postgres\n");

            info!("Environment variables:");
            info!("  REDIS_URL      - Redis connection URL (default: redis://localhost:6379)");
            info!("  DATABASE_URL   - PostgreSQL connection URL");
            info!("  CLICKHOUSE_URL - ClickHouse HTTP URL (default: http://localhost:8123)");

            return Ok(());
        }
    };

    run_simulation(config).await
}

async fn run_simulation(config: MixedBackendConfig) -> Result<(), Box<dyn std::error::Error>> {
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║  ACTEON MIXED BACKENDS SIMULATION                            ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    info!("Configuration: {}\n", config.name);

    // Create state backend
    let (state, lock): (Arc<dyn StateStore>, Arc<dyn DistributedLock>) = match config.state {
        StateBackend::Memory => {
            info!("→ State backend: Memory (in-process)");
            let state = Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>;
            let lock = Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>;
            (state, lock)
        }
        #[cfg(feature = "redis")]
        StateBackend::Redis { url } => {
            info!("→ State backend: Redis at {}", url);
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
    info!("  ✓ State store connected");

    // Create audit backend
    let audit: Arc<dyn AuditStore> = match config.audit {
        #[cfg(feature = "postgres")]
        AuditBackend::Postgres { url } => {
            info!(
                "→ Audit backend: PostgreSQL at {}",
                url.split('@').last().unwrap_or(&url)
            );
            let audit_config = PostgresAuditConfig::new(&url).with_prefix("acteon_mixed_");
            Arc::new(PostgresAuditStore::new(&audit_config).await?)
        }
        #[cfg(feature = "clickhouse")]
        AuditBackend::ClickHouse { url } => {
            info!("→ Audit backend: ClickHouse at {}", url);
            let audit_config = ClickHouseAuditConfig::new(&url).with_prefix("acteon_mixed_");
            Arc::new(ClickHouseAuditStore::new(&audit_config).await?)
        }
    };
    info!("  ✓ Audit store connected\n");

    // Parse rules
    let frontend = YamlFrontend;
    let mut rules = frontend.parse(DEDUP_RULE)?;
    rules.extend(frontend.parse(SUPPRESSION_RULE)?);
    rules.extend(frontend.parse(THROTTLE_RULE)?);

    info!("✓ Loaded {} rules", rules.len());
    for rule in &rules {
        info!("  - {}: {:?}", rule.name, rule.action);
    }
    info!("");

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

    info!("✓ Gateway built with mixed backends\n");

    // =========================================================================
    // SCENARIO 1: Basic Execution with Cross-Backend Audit
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: BASIC EXECUTION (State + Audit Integration)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    info!("→ Dispatching welcome email...");
    let outcome = gateway.dispatch(action.clone(), None).await?;
    info!("  Outcome: {:?}", outcome);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let audit_record = audit.get_by_action_id(&action.id.to_string()).await?;
    if let Some(record) = audit_record {
        info!("  ✓ Audit recorded in separate backend:");
        info!(
            "    Outcome: {}, Duration: {}ms",
            record.outcome, record.duration_ms
        );
    }

    // =========================================================================
    // SCENARIO 2: Deduplication with State, Audit in Different Store
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: DEDUPLICATION (State handles dedup, Audit records)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    info!("→ Dispatching FIRST notification...");
    let outcome1 = gateway.dispatch(notify1.clone(), None).await?;
    info!("  Outcome: {:?}", outcome1);

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

    info!("→ Dispatching DUPLICATE notification...");
    let outcome2 = gateway.dispatch(notify2.clone(), None).await?;
    info!("  Outcome: {:?}", outcome2);

    info!(
        "  Email provider calls: {} (should be 1)",
        email_provider.call_count()
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify both are audited
    let record1 = audit.get_by_action_id(&notify1.id.to_string()).await?;
    let record2 = audit.get_by_action_id(&notify2.id.to_string()).await?;

    info!("\n  Audit trail verification:");
    if let Some(r) = record1 {
        info!(
            "    First action:  {} ({})",
            r.outcome,
            r.action_id[..8].to_string()
        );
    }
    if let Some(r) = record2 {
        info!(
            "    Second action: {} ({})",
            r.outcome,
            r.action_id[..8].to_string()
        );
    }

    // =========================================================================
    // SCENARIO 3: Throttling with Audit Trail
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: THROTTLING (Rate limit in state, all audited)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    push_provider.clear();

    info!("→ Dispatching 5 alert actions (throttle limit: 3/10s)...\n");

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
        info!("  Alert #{}: {:?}", i, outcome);
    }

    let executed = alert_outcomes
        .iter()
        .filter(|o| matches!(o, acteon_core::ActionOutcome::Executed(_)))
        .count();
    let throttled = alert_outcomes
        .iter()
        .filter(|o| matches!(o, acteon_core::ActionOutcome::Throttled { .. }))
        .count();

    info!(
        "\n  Results: {} executed, {} throttled",
        executed, throttled
    );
    info!(
        "  Push provider calls: {} (should be 3)",
        push_provider.call_count()
    );

    // =========================================================================
    // SCENARIO 4: Multi-Provider Dispatch
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 4: MULTI-PROVIDER (Different channels, unified audit)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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
        info!("  {} campaign: {:?}", provider.to_uppercase(), outcome);
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Query unified audit by tenant
    let tenant2_query = AuditQuery {
        tenant: Some("tenant-2".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let tenant2_page = audit.query(&tenant2_query).await?;
    info!(
        "\n  Unified audit for tenant-2: {} records",
        tenant2_page.total.unwrap_or(0)
    );
    for record in &tenant2_page.records {
        info!(
            "    - {} via {} ({})",
            record.action_type, record.provider, record.outcome
        );
    }

    // =========================================================================
    // SCENARIO 5: Concurrent Dispatch with Mixed Backends
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 5: CONCURRENT DISPATCH (Stress test mixed backends)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    let gateway_arc = Arc::new(gateway);
    let mut handles = vec![];

    info!("→ Dispatching 20 concurrent actions with same dedup key...");

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
            Ok(other) => info!("  Unexpected: {:?}", other),
            Err(e) => info!("  Error: {}", e),
        }
    }

    info!("  Executed: {}, Deduplicated: {}", executed, deduplicated);
    info!("  Email provider calls: {}", email_provider.call_count());

    // =========================================================================
    // SCENARIO 6: Throughput Benchmark
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 6: THROUGHPUT BENCHMARK");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    info!("→ Dispatching {} actions sequentially...", batch_size);
    let start = std::time::Instant::now();

    for action in actions {
        gateway_arc.dispatch(action, None).await?;
    }

    let elapsed = start.elapsed();
    let throughput = batch_size as f64 / elapsed.as_secs_f64();

    info!("  Completed in: {:?}", elapsed);
    info!("  Throughput: {:.0} actions/sec", throughput);

    // Wait for audit writes
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Verify all audited
    let bench_query = AuditQuery {
        namespace: Some("benchmark".to_string()),
        limit: Some(1),
        ..Default::default()
    };
    let bench_page = audit.query(&bench_query).await?;
    info!("  Audit records created: {}", bench_page.total.unwrap_or(0));

    // =========================================================================
    // SCENARIO 7: Failure Injection
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 7: FAILURE INJECTION (Provider errors, retries)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // 7a: FailingProvider - always fails
    info!("  7a) FailingProvider (always fails):");
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

    info!("      → Dispatching to failing webhook provider...");
    let result = gw_with_failing.dispatch(webhook_action.clone(), None).await;
    match &result {
        Ok(outcome) => info!("      Outcome: {:?}", outcome),
        Err(e) => info!("      Error (expected): {}", e),
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check audit - failures should still be recorded
    let fail_audit = audit
        .get_by_action_id(&webhook_action.id.to_string())
        .await?;
    if let Some(record) = fail_audit {
        info!("      ✓ Failure audited: outcome={}", record.outcome);
    }

    gw_with_failing.shutdown().await;

    // 7b: FailingProvider - fail then recover
    info!("\n  7b) FailingProvider with recovery (fail first 2, then succeed):");
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
                info!("      Attempt #{}: SUCCESS", i)
            }
            Ok(outcome) => info!("      Attempt #{}: {:?}", i, outcome),
            Err(e) => info!("      Attempt #{}: FAILED - {}", i, e),
        }
    }
    info!(
        "      Provider call count: {}",
        recovering_provider.call_count()
    );

    gw_recovering.shutdown().await;

    // 7c: RecordingProvider with FailureMode
    info!("\n  7c) RecordingProvider with FailureMode::EveryN(3):");
    let flaky_provider =
        Arc::new(RecordingProvider::new("flaky").with_failure_mode(FailureMode::EveryN(3)));

    let gw_flaky = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .audit(audit.clone())
        .provider(flaky_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    info!("      → Dispatching 6 actions (every 3rd fails):");
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
    info!(
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
    info!("\n  7d) RecordingProvider with simulated latency (50ms):");
    let slow_provider =
        Arc::new(RecordingProvider::new("slow").with_delay(std::time::Duration::from_millis(50)));

    let gw_slow = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .provider(slow_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    info!("      → Dispatching 5 actions with latency...");
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
    info!("      Total time: {:?} (expected ~250ms)", elapsed);
    info!("      Avg latency: {:?}/action", elapsed / 5);

    gw_slow.shutdown().await;

    // 7e: RecordingProvider with custom response
    info!("\n  7e) RecordingProvider with custom response function:");
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
            info!(
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
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 8: REROUTING (If-Then-Else Logic)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    info!("  Rules loaded:");
    info!("    - urgent email → SMS");
    info!("    - premium-tier transaction → premium provider");
    info!("    - after-hours email → Slack");
    info!("    - enterprise tenant → dedicated provider");
    info!("    - retry email → email-backup\n");

    // 8a: Normal email (no reroute)
    info!("  8a) Normal email (no reroute condition):");
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
    info!("      → Original provider: email");
    info!("      → Executed by: email (no reroute)");
    info!(
        "      Email calls: {}, SMS calls: {}",
        email_reroute.call_count(),
        sms_reroute.call_count()
    );

    // 8b: Urgent email → SMS
    info!("\n  8b) Urgent email → rerouted to SMS:");
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
    info!("      → Original provider: email");
    info!("      → Rerouted to: sms (because priority=urgent)");
    info!(
        "      Email calls: {}, SMS calls: {}",
        email_reroute.call_count(),
        sms_reroute.call_count()
    );

    // Verify audit shows reroute
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    if let Some(record) = audit.get_by_action_id(&urgent_email.id.to_string()).await? {
        info!(
            "      Audit: verdict={}, provider={}",
            record.verdict, record.provider
        );
    }

    // 8c: Premium-tier transaction → premium provider
    info!("\n  8c) Premium-tier transaction → premium provider:");
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
    info!("      → Original provider: standard");
    info!("      → Rerouted to: premium (because payload.tier=premium)");
    info!("      Premium calls: {}", premium_reroute.call_count());

    // 8d: Standard-tier transaction (no reroute)
    info!("\n  8d) Standard-tier transaction → no reroute:");
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
    info!("      → Original provider: standard (no reroute, tier != premium)");
    info!(
        "      Premium calls: {} (should be 0)",
        premium_reroute.call_count()
    );

    // 8e: After-hours email → Slack
    info!("\n  8e) After-hours email → rerouted to Slack:");
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
    info!("      → Original provider: email");
    info!("      → Rerouted to: slack (because after_hours=true)");
    info!(
        "      Email calls: {}, Slack calls: {}",
        email_reroute.call_count(),
        slack_reroute.call_count()
    );

    // 8f: Enterprise tier → dedicated
    info!("\n  8f) Enterprise tenant → dedicated provider:");
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
    info!("      → Original provider: email");
    info!("      → Rerouted to: dedicated (because metadata.tier=enterprise)");
    info!("      Dedicated calls: {}", dedicated_reroute.call_count());

    // 8g: Retry fallback → email-backup
    info!("\n  8g) Retry email → backup provider:");
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
    info!("      → Original provider: email");
    info!("      → Rerouted to: email-backup (because is_retry=true)");
    info!(
        "      Email calls: {}, Backup calls: {}",
        email_reroute.call_count(),
        email_backup_reroute.call_count()
    );

    // 8h: Multiple conditions - urgent + after_hours
    info!("\n  8h) Multiple matching rules (priority wins):");
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
    info!("      → Matches: reroute-urgent-to-sms (priority=1), reroute-after-hours (priority=3)");
    info!("      → Winner: SMS (higher priority rule)");
    info!(
        "      SMS calls: {}, Slack calls: {}",
        sms_reroute.call_count(),
        slack_reroute.call_count()
    );

    // Summary
    info!("\n  Rerouting Summary:");
    info!(
        "    Total email provider calls: {}",
        email_reroute.call_count()
    );
    info!("    Total SMS provider calls: {}", sms_reroute.call_count());
    info!(
        "    Total Slack provider calls: {}",
        slack_reroute.call_count()
    );
    info!(
        "    Total Premium provider calls: {}",
        premium_reroute.call_count()
    );
    info!(
        "    Total Dedicated provider calls: {}",
        dedicated_reroute.call_count()
    );
    info!(
        "    Total Backup provider calls: {}",
        email_backup_reroute.call_count()
    );

    gw_reroute.shutdown().await;

    // =========================================================================
    // Summary
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  AUDIT TRAIL SUMMARY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    info!("  Total audit records: {}", all_page.total.unwrap_or(0));
    info!("    - Executed: {}", executed_count);
    info!("    - Suppressed: {}", suppressed_count);
    info!("    - Deduplicated: {}", deduplicated_count);
    info!("    - Throttled: {}", throttled_count);

    // Unique tenants
    let tenants: std::collections::HashSet<_> =
        all_page.records.iter().map(|r| &r.tenant).collect();
    info!("\n  Tenants tracked: {:?}", tenants);

    // Unique providers
    let providers: std::collections::HashSet<_> =
        all_page.records.iter().map(|r| &r.provider).collect();
    info!("  Providers tracked: {:?}", providers);

    gateway_arc.shutdown().await;
    info!("\n✓ Gateway shut down gracefully\n");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║           MIXED BACKENDS SIMULATION COMPLETE                 ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
