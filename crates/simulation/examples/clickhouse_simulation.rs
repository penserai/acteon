//! Demonstration of Acteon with ClickHouse backend including audit trail.
//!
//! This example runs the full Acteon gateway against a real ClickHouse instance
//! with end-to-end audit trail verification.
//!
//! Prerequisites:
//!   docker run -d --name acteon-clickhouse -p 8123:8123 -p 9000:9000 \
//!     -e CLICKHOUSE_PASSWORD="" clickhouse/clickhouse-server:latest
//!
//! Run with:
//!   cargo run -p acteon-simulation --example clickhouse_simulation --features clickhouse
//!
//! Note: ClickHouse uses ReplacingMergeTree for state storage which provides
//! eventual consistency. The distributed lock implementation is best-effort
//! and may have brief windows where two processes believe they hold the same lock.

use std::sync::Arc;

use acteon_audit::AuditQuery;
use acteon_audit::store::AuditStore;
use acteon_audit_clickhouse::{ClickHouseAuditConfig, ClickHouseAuditStore};
use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::RecordingProvider;

// Import ClickHouse backends
use acteon_state_clickhouse::{ClickHouseConfig, ClickHouseDistributedLock, ClickHouseStateStore};
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║  ACTEON SIMULATION WITH CLICKHOUSE + AUDIT TRAIL             ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // Configure ClickHouse connection
    let clickhouse_url =
        std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".to_string());

    // State backend config
    let state_config = ClickHouseConfig {
        url: clickhouse_url.clone(),
        database: "default".to_string(),
        table_prefix: "acteon_sim_".to_string(),
    };

    // Audit backend config
    let audit_config = ClickHouseAuditConfig::new(&clickhouse_url)
        .with_database("default")
        .with_prefix("acteon_sim_");

    info!("→ Connecting to ClickHouse at {}...", clickhouse_url);

    // Create ClickHouse-backed state store, lock, and AUDIT store
    let state = Arc::new(ClickHouseStateStore::new(state_config.clone()).await?);
    let lock = Arc::new(ClickHouseDistributedLock::new(state_config.clone()).await?);
    let audit = Arc::new(ClickHouseAuditStore::new(&audit_config).await?);

    info!("✓ Connected to ClickHouse");
    info!("✓ State tables: default.acteon_sim_state, default.acteon_sim_locks");
    info!("✓ Audit table: default.acteon_sim_audit");
    info!("");

    // Parse rules
    let frontend = YamlFrontend;
    let mut rules = frontend.parse(DEDUP_RULE)?;
    rules.extend(frontend.parse(SUPPRESSION_RULE)?);

    info!("✓ Loaded {} rules", rules.len());
    for rule in &rules {
        info!("  - {}: {:?}", rule.name, rule.action);
    }
    info!("");

    // Create recording providers
    let email_provider = Arc::new(RecordingProvider::new("email"));
    let sms_provider = Arc::new(RecordingProvider::new("sms"));

    // Build the gateway with ClickHouse backends AND audit store
    let gateway = GatewayBuilder::new()
        .state(state.clone())
        .lock(lock.clone())
        .audit(audit.clone())
        .audit_ttl_seconds(3600) // Audit records expire after 1 hour
        .audit_store_payload(true) // Store action payloads in audit
        .rules(rules)
        .provider(email_provider.clone() as Arc<dyn DynProvider>)
        .provider(sms_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    info!("✓ Gateway built with ClickHouse state, lock, and AUDIT backends\n");

    // =========================================================================
    // DEMO 1: Action Execution with Audit Trail
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: ACTION EXECUTION WITH AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let action1 = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({
            "user_id": "user-123",
            "message": "You have a new message",
        }),
    )
    .with_dedup_key("ch-audit-test-1");

    info!("→ Dispatching notification action...");
    let outcome1 = gateway.dispatch(action1.clone(), None).await?;
    info!("  Outcome: {:?}", outcome1);
    info!("  Action ID: {}", action1.id);

    // Wait for audit to be recorded (it's async)
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify audit trail
    info!("\n→ Querying audit trail for action {}...", action1.id);
    let audit_record = audit.get_by_action_id(&action1.id.to_string()).await?;

    if let Some(record) = audit_record {
        info!("  ✓ Audit record found!");
        info!("    Record ID: {}", record.id);
        info!("    Namespace: {}", record.namespace);
        info!("    Tenant: {}", record.tenant);
        info!("    Provider: {}", record.provider);
        info!("    Action Type: {}", record.action_type);
        info!("    Verdict: {}", record.verdict);
        info!("    Outcome: {}", record.outcome);
        info!("    Duration: {}ms", record.duration_ms);
        if let Some(ref payload) = record.action_payload {
            info!("    Payload: {}", payload);
        }
    } else {
        info!("  ✗ No audit record found!");
    }

    // =========================================================================
    // DEMO 2: Suppressed Action Audit Trail
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: SUPPRESSED ACTION AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let spam_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "spam",
        serde_json::json!({
            "subject": "Buy now!!!",
        }),
    );

    info!("→ Dispatching SPAM action...");
    let outcome = gateway.dispatch(spam_action.clone(), None).await?;
    info!("  Outcome: {:?}", outcome);
    info!("  Action ID: {}", spam_action.id);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    info!("\n→ Querying audit trail for suppressed action...");
    let audit_record = audit.get_by_action_id(&spam_action.id.to_string()).await?;

    if let Some(record) = audit_record {
        info!("  ✓ Audit record found!");
        info!("    Verdict: {}", record.verdict);
        info!("    Outcome: {}", record.outcome);
        info!("    Matched Rule: {:?}", record.matched_rule);
        info!("    (Suppressed actions are still audited!)");
    }

    // =========================================================================
    // DEMO 3: Deduplicated Action Audit Trail
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: DEDUPLICATED ACTION AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let dup_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({
            "user_id": "user-123",
            "message": "Duplicate message",
        }),
    )
    .with_dedup_key("ch-audit-test-1"); // Same dedup key as action1

    info!("→ Dispatching DUPLICATE action (same dedup_key)...");
    let outcome = gateway.dispatch(dup_action.clone(), None).await?;
    info!("  Outcome: {:?}", outcome);
    info!("  Action ID: {}", dup_action.id);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    info!("\n→ Querying audit trail for deduplicated action...");
    let audit_record = audit.get_by_action_id(&dup_action.id.to_string()).await?;

    if let Some(record) = audit_record {
        info!("  ✓ Audit record found!");
        info!("    Verdict: {}", record.verdict);
        info!("    Outcome: {}", record.outcome);
        info!("    Matched Rule: {:?}", record.matched_rule);
        info!("    (Deduplicated actions are audited with 'deduplicated' outcome!)");
    }

    // =========================================================================
    // DEMO 4: Query Audit Trail with Filters
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: QUERY AUDIT TRAIL WITH FILTERS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Query all records for tenant-1
    let query = AuditQuery {
        tenant: Some("tenant-1".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    info!("→ Querying all audit records for tenant-1...");
    let page = audit.query(&query).await?;
    info!("  Total records: {}", page.total);
    info!("  Records in page: {}\n", page.records.len());

    for record in &page.records {
        info!(
            "  - {} | {} | {} | {}",
            &record.action_id[..8],
            record.action_type,
            record.outcome,
            record.dispatched_at.format("%H:%M:%S")
        );
    }

    // Query suppressed actions only
    info!("\n→ Querying only SUPPRESSED actions...");
    let suppressed_query = AuditQuery {
        outcome: Some("suppressed".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let suppressed_page = audit.query(&suppressed_query).await?;
    info!("  Suppressed actions: {}", suppressed_page.total);

    // Query executed actions only
    info!("\n→ Querying only EXECUTED actions...");
    let executed_query = AuditQuery {
        outcome: Some("executed".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let executed_page = audit.query(&executed_query).await?;
    info!("  Executed actions: {}", executed_page.total);

    // =========================================================================
    // DEMO 5: Concurrent Dispatch (Eventual Consistency Warning)
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 5: CONCURRENT DISPATCH (EVENTUAL CONSISTENCY)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    // Note: ClickHouse's lock is best-effort due to its append-only nature
    info!("→ Simulating 10 concurrent dispatches with SAME dedup_key...");
    info!("  (ClickHouse locking is best-effort, eventual consistency)\n");

    let gateway_arc = Arc::new(gateway);
    let mut handles = vec![];

    for i in 0..10 {
        let gw = Arc::clone(&gateway_arc);
        let handle = tokio::spawn(async move {
            let action = Action::new(
                "notifications",
                "tenant-1",
                "email",
                "notify",
                serde_json::json!({
                    "worker": i,
                    "message": "Concurrent test",
                }),
            )
            .with_dedup_key("ch-concurrent-dedup-key");

            gw.dispatch(action, None).await
        });
        handles.push(handle);
    }

    // Wait for all to complete
    let mut executed = 0;
    let mut deduplicated = 0;
    let mut failed = 0;

    for handle in handles {
        match handle.await? {
            Ok(acteon_core::ActionOutcome::Executed(_)) => executed += 1,
            Ok(acteon_core::ActionOutcome::Deduplicated) => deduplicated += 1,
            Ok(other) => info!("  Unexpected outcome: {:?}", other),
            Err(e) => {
                info!("  Error: {}", e);
                failed += 1;
            }
        }
    }

    info!("  Results:");
    info!("    Executed: {}", executed);
    info!("    Deduplicated: {}", deduplicated);
    info!("    Failed: {}", failed);
    info!(
        "    Email provider called: {} times",
        email_provider.call_count()
    );
    info!("\n  (ClickHouse may execute more than 1 due to eventual consistency)");

    // =========================================================================
    // DEMO 6: Throughput with Audit
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 6: THROUGHPUT WITH AUDIT ENABLED");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    let actions: Vec<Action> = (0..100)
        .map(|i| {
            Action::new(
                "bulk",
                "tenant-1",
                "email",
                "bulk_send",
                serde_json::json!({"recipient_id": i}),
            )
        })
        .collect();

    info!("→ Dispatching 100 actions with audit enabled...");
    let start = std::time::Instant::now();

    for action in actions {
        gateway_arc.dispatch(action, None).await?;
    }

    let elapsed = start.elapsed();
    info!("  Completed in: {:?}", elapsed);
    info!(
        "  Throughput: {:.0} actions/sec",
        100.0 / elapsed.as_secs_f64()
    );

    // Wait for audit records to be written
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Verify audit records were created
    let bulk_query = AuditQuery {
        action_type: Some("bulk_send".to_string()),
        limit: Some(1),
        ..Default::default()
    };
    let bulk_page = audit.query(&bulk_query).await?;
    info!("  Audit records created: {}", bulk_page.total);

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

    info!("  Total audit records: {}", all_page.total);
    info!("    - Executed: {}", executed_count);
    info!("    - Suppressed: {}", suppressed_count);
    info!("    - Deduplicated: {}", deduplicated_count);

    info!("\n  You can query the audit trail with clickhouse-client:");
    info!("    docker exec acteon-clickhouse clickhouse-client \\");
    info!("      -q 'SELECT action_type, outcome, matched_rule, dispatched_at");
    info!("          FROM default.acteon_sim_audit ORDER BY dispatched_at DESC LIMIT 10'");

    // Cleanup
    gateway_arc.shutdown().await;
    info!("\n✓ Gateway shut down gracefully\n");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║         CLICKHOUSE + AUDIT DEMO COMPLETE                     ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
