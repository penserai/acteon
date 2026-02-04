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
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  ACTEON SIMULATION WITH CLICKHOUSE + AUDIT TRAIL             ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

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

    println!("→ Connecting to ClickHouse at {}...", clickhouse_url);

    // Create ClickHouse-backed state store, lock, and AUDIT store
    let state = Arc::new(ClickHouseStateStore::new(state_config.clone()).await?);
    let lock = Arc::new(ClickHouseDistributedLock::new(state_config.clone()).await?);
    let audit = Arc::new(ClickHouseAuditStore::new(&audit_config).await?);

    println!("✓ Connected to ClickHouse");
    println!("✓ State tables: default.acteon_sim_state, default.acteon_sim_locks");
    println!("✓ Audit table: default.acteon_sim_audit");
    println!();

    // Parse rules
    let frontend = YamlFrontend;
    let mut rules = frontend.parse(DEDUP_RULE)?;
    rules.extend(frontend.parse(SUPPRESSION_RULE)?);

    println!("✓ Loaded {} rules", rules.len());
    for rule in &rules {
        println!("  - {}: {:?}", rule.name, rule.action);
    }
    println!();

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

    println!("✓ Gateway built with ClickHouse state, lock, and AUDIT backends\n");

    // =========================================================================
    // DEMO 1: Action Execution with Audit Trail
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 1: ACTION EXECUTION WITH AUDIT TRAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    println!("→ Dispatching notification action...");
    let outcome1 = gateway.dispatch(action1.clone(), None).await?;
    println!("  Outcome: {:?}", outcome1);
    println!("  Action ID: {}", action1.id);

    // Wait for audit to be recorded (it's async)
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify audit trail
    println!("\n→ Querying audit trail for action {}...", action1.id);
    let audit_record = audit.get_by_action_id(&action1.id.to_string()).await?;

    if let Some(record) = audit_record {
        println!("  ✓ Audit record found!");
        println!("    Record ID: {}", record.id);
        println!("    Namespace: {}", record.namespace);
        println!("    Tenant: {}", record.tenant);
        println!("    Provider: {}", record.provider);
        println!("    Action Type: {}", record.action_type);
        println!("    Verdict: {}", record.verdict);
        println!("    Outcome: {}", record.outcome);
        println!("    Duration: {}ms", record.duration_ms);
        if let Some(ref payload) = record.action_payload {
            println!("    Payload: {}", payload);
        }
    } else {
        println!("  ✗ No audit record found!");
    }

    // =========================================================================
    // DEMO 2: Suppressed Action Audit Trail
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 2: SUPPRESSED ACTION AUDIT TRAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let spam_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "spam",
        serde_json::json!({
            "subject": "Buy now!!!",
        }),
    );

    println!("→ Dispatching SPAM action...");
    let outcome = gateway.dispatch(spam_action.clone(), None).await?;
    println!("  Outcome: {:?}", outcome);
    println!("  Action ID: {}", spam_action.id);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    println!("\n→ Querying audit trail for suppressed action...");
    let audit_record = audit.get_by_action_id(&spam_action.id.to_string()).await?;

    if let Some(record) = audit_record {
        println!("  ✓ Audit record found!");
        println!("    Verdict: {}", record.verdict);
        println!("    Outcome: {}", record.outcome);
        println!("    Matched Rule: {:?}", record.matched_rule);
        println!("    (Suppressed actions are still audited!)");
    }

    // =========================================================================
    // DEMO 3: Deduplicated Action Audit Trail
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 3: DEDUPLICATED ACTION AUDIT TRAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    println!("→ Dispatching DUPLICATE action (same dedup_key)...");
    let outcome = gateway.dispatch(dup_action.clone(), None).await?;
    println!("  Outcome: {:?}", outcome);
    println!("  Action ID: {}", dup_action.id);

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    println!("\n→ Querying audit trail for deduplicated action...");
    let audit_record = audit.get_by_action_id(&dup_action.id.to_string()).await?;

    if let Some(record) = audit_record {
        println!("  ✓ Audit record found!");
        println!("    Verdict: {}", record.verdict);
        println!("    Outcome: {}", record.outcome);
        println!("    Matched Rule: {:?}", record.matched_rule);
        println!("    (Deduplicated actions are audited with 'deduplicated' outcome!)");
    }

    // =========================================================================
    // DEMO 4: Query Audit Trail with Filters
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 4: QUERY AUDIT TRAIL WITH FILTERS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Query all records for tenant-1
    let query = AuditQuery {
        tenant: Some("tenant-1".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    println!("→ Querying all audit records for tenant-1...");
    let page = audit.query(&query).await?;
    println!("  Total records: {}", page.total);
    println!("  Records in page: {}\n", page.records.len());

    for record in &page.records {
        println!(
            "  - {} | {} | {} | {}",
            &record.action_id[..8],
            record.action_type,
            record.outcome,
            record.dispatched_at.format("%H:%M:%S")
        );
    }

    // Query suppressed actions only
    println!("\n→ Querying only SUPPRESSED actions...");
    let suppressed_query = AuditQuery {
        outcome: Some("suppressed".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let suppressed_page = audit.query(&suppressed_query).await?;
    println!("  Suppressed actions: {}", suppressed_page.total);

    // Query executed actions only
    println!("\n→ Querying only EXECUTED actions...");
    let executed_query = AuditQuery {
        outcome: Some("executed".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let executed_page = audit.query(&executed_query).await?;
    println!("  Executed actions: {}", executed_page.total);

    // =========================================================================
    // DEMO 5: Concurrent Dispatch (Eventual Consistency Warning)
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 5: CONCURRENT DISPATCH (EVENTUAL CONSISTENCY)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    // Note: ClickHouse's lock is best-effort due to its append-only nature
    println!("→ Simulating 10 concurrent dispatches with SAME dedup_key...");
    println!("  (ClickHouse locking is best-effort, eventual consistency)\n");

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
            Ok(other) => println!("  Unexpected outcome: {:?}", other),
            Err(e) => {
                println!("  Error: {}", e);
                failed += 1;
            }
        }
    }

    println!("  Results:");
    println!("    Executed: {}", executed);
    println!("    Deduplicated: {}", deduplicated);
    println!("    Failed: {}", failed);
    println!(
        "    Email provider called: {} times",
        email_provider.call_count()
    );
    println!("\n  (ClickHouse may execute more than 1 due to eventual consistency)");

    // =========================================================================
    // DEMO 6: Throughput with Audit
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 6: THROUGHPUT WITH AUDIT ENABLED");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    println!("→ Dispatching 100 actions with audit enabled...");
    let start = std::time::Instant::now();

    for action in actions {
        gateway_arc.dispatch(action, None).await?;
    }

    let elapsed = start.elapsed();
    println!("  Completed in: {:?}", elapsed);
    println!(
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
    println!("  Audit records created: {}", bulk_page.total);

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

    println!("  Total audit records: {}", all_page.total);
    println!("    - Executed: {}", executed_count);
    println!("    - Suppressed: {}", suppressed_count);
    println!("    - Deduplicated: {}", deduplicated_count);

    println!("\n  You can query the audit trail with clickhouse-client:");
    println!("    docker exec acteon-clickhouse clickhouse-client \\");
    println!("      -q 'SELECT action_type, outcome, matched_rule, dispatched_at");
    println!("          FROM default.acteon_sim_audit ORDER BY dispatched_at DESC LIMIT 10'");

    // Cleanup
    gateway_arc.shutdown().await;
    println!("\n✓ Gateway shut down gracefully\n");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║         CLICKHOUSE + AUDIT DEMO COMPLETE                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
