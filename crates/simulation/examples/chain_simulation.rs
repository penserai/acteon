//! Simulation of task chains with audit trail verification.
//!
//! Demonstrates:
//! 1. A successful multi-step chain (search -> summarize -> email)
//! 2. A chain with a failing step and Abort policy
//! 3. A chain with a failing step and Skip policy
//! 4. Chain cancellation
//! 5. Chain audit trail queries (step + terminal records)
//!
//! Run with: `cargo run -p acteon-simulation --example chain_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_audit::record::AuditQuery;
use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::chain::{ChainConfig, ChainStepConfig, StepFailurePolicy};
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

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

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("           ACTEON CHAIN SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // DEMO 1: Successful Multi-Step Chain
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 1: SUCCESSFUL MULTI-STEP CHAIN");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let search_provider = Arc::new(RecordingProvider::new("search-api").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "results": [
                {"title": "Rust async primer", "url": "https://example.com/1"},
                {"title": "Tokio guide", "url": "https://example.com/2"},
            ]
        })))
    }));
    let summarize_provider = Arc::new(RecordingProvider::new("llm-api").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "summary": "Rust's async model uses Futures with Tokio as the primary runtime."
        })))
    }));
    let email_provider = Arc::new(RecordingProvider::new("email"));

    let chain_config = ChainConfig::new("search-summarize-email")
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
            serde_json::json!({
                "text": "{{prev.response_body}}",
                "max_length": 200
            }),
        ))
        .with_step(ChainStepConfig::new(
            "notify",
            "email",
            "send_email",
            serde_json::json!({
                "to": "{{origin.payload.email}}",
                "subject": "Research results for: {{origin.payload.query}}",
                "body": "{{prev.response_body.summary}}"
            }),
        ))
        .with_timeout(60);

    let rules = parse_rules(CHAIN_RULE);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&search_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&summarize_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&email_provider) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "research",
        "tenant-1",
        "search-api",
        "research",
        serde_json::json!({
            "query": "rust async programming",
            "email": "dev@example.com"
        }),
    );

    println!("  Dispatching research action...");
    let outcome = gateway.dispatch(action, None).await?;

    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            total_steps,
            first_step,
        } => {
            println!("  Chain started: {chain_name}");
            println!("    chain_id:    {chain_id}");
            println!("    total_steps: {total_steps}");
            println!("    first_step:  {first_step}");
            chain_id.clone()
        }
        other => {
            println!("  Unexpected outcome: {other:?}");
            return Ok(());
        }
    };

    // Advance through all 3 steps.
    for step in 0..3 {
        gateway
            .advance_chain("research", "tenant-1", &chain_id)
            .await?;
        println!("  Step {step} advanced");
    }

    // Wait for async audit writes.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify providers were called.
    println!("\n  Provider call counts:");
    println!("    search-api: {}", search_provider.call_count());
    println!("    llm-api:    {}", summarize_provider.call_count());
    println!("    email:      {}", email_provider.call_count());

    // Query audit records for this chain.
    let page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id.clone()),
            ..Default::default()
        })
        .await?;

    println!("\n  Audit records for chain {chain_id}: {}", page.total);
    for rec in &page.records {
        println!(
            "    [{:>22}] provider={:<12} outcome={}",
            rec.outcome, rec.provider, rec.action_type
        );
    }

    // Verify the chain status.
    let chain_state = gateway
        .get_chain_status("research", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");
    println!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );

    // Verify we have step + terminal audit records.
    let step_records: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.outcome.starts_with("chain_step"))
        .collect();
    let terminal_records: Vec<_> = page
        .records
        .iter()
        .filter(|r| {
            r.outcome == "chain_completed"
                || r.outcome == "chain_failed"
                || r.outcome == "chain_timed_out"
                || r.outcome == "chain_cancelled"
        })
        .collect();
    println!("  Step audit records:     {}", step_records.len());
    println!("  Terminal audit records:  {}", terminal_records.len());
    assert_eq!(step_records.len(), 3, "expected 3 step records");
    assert_eq!(terminal_records.len(), 1, "expected 1 terminal record");
    assert_eq!(terminal_records[0].outcome, "chain_completed");

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 2: Chain with Failing Step (Abort)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 2: CHAIN WITH FAILING STEP (ABORT POLICY)");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let ok_provider = Arc::new(RecordingProvider::new("step-ok").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"ok": true}),
        ))
    }));
    let fail_provider =
        Arc::new(RecordingProvider::new("step-fail").with_failure_mode(FailureMode::Always));
    let unreachable_provider = Arc::new(RecordingProvider::new("step-unreachable"));

    let chain_config = ChainConfig::new("abort-chain")
        .with_step(ChainStepConfig::new(
            "first",
            "step-ok",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "second-fails",
            "step-fail",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "third-unreachable",
            "step-unreachable",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let abort_rule: &str = r#"
rules:
  - name: trigger-abort-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "trigger_abort"
    action:
      type: chain
      chain: abort-chain
"#;
    let rules = parse_rules(abort_rule);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&ok_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&fail_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&unreachable_provider) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "step-ok",
        "trigger_abort",
        serde_json::json!({}),
    );

    println!("  Dispatching action to trigger abort-chain...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => {
            println!("  Unexpected outcome: {other:?}");
            return Ok(());
        }
    };

    // Advance step 0 (succeeds), then step 1 (fails -> abort).
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 0 (first): advanced OK");
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 1 (second-fails): failed -> chain aborted");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let chain_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");
    println!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(chain_state.status, acteon_core::chain::ChainStatus::Failed);

    println!("  Provider call counts:");
    println!("    step-ok:          {}", ok_provider.call_count());
    println!("    step-fail:        {}", fail_provider.call_count());
    println!(
        "    step-unreachable: {} (never reached)",
        unreachable_provider.call_count()
    );
    unreachable_provider.assert_not_called();

    let page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id.clone()),
            ..Default::default()
        })
        .await?;
    println!("\n  Audit records for chain: {}", page.total);
    for rec in &page.records {
        println!(
            "    [{:>22}] provider={:<16} action_type={}",
            rec.outcome, rec.provider, rec.action_type
        );
    }

    let failed_records: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.outcome == "chain_failed")
        .collect();
    assert_eq!(
        failed_records.len(),
        1,
        "expected 1 chain_failed terminal record"
    );

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 3: Chain with Failing Step (Skip)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 3: CHAIN WITH FAILING STEP (SKIP POLICY)");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let ok_provider = Arc::new(RecordingProvider::new("step-a").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"result": "done"}),
        ))
    }));
    let fail_provider =
        Arc::new(RecordingProvider::new("step-b").with_failure_mode(FailureMode::Always));
    let final_provider = Arc::new(RecordingProvider::new("step-c").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"final": true}),
        ))
    }));

    let chain_config = ChainConfig::new("skip-chain")
        .with_step(ChainStepConfig::new(
            "first",
            "step-a",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(
            ChainStepConfig::new(
                "second-skippable",
                "step-b",
                "do_thing",
                serde_json::json!({}),
            )
            .with_on_failure(StepFailurePolicy::Skip),
        )
        .with_step(ChainStepConfig::new(
            "third",
            "step-c",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let skip_rule: &str = r#"
rules:
  - name: trigger-skip-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "trigger_skip"
    action:
      type: chain
      chain: skip-chain
"#;
    let rules = parse_rules(skip_rule);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&ok_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&fail_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&final_provider) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "step-a",
        "trigger_skip",
        serde_json::json!({}),
    );

    println!("  Dispatching action to trigger skip-chain...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => {
            println!("  Unexpected outcome: {other:?}");
            return Ok(());
        }
    };

    // Advance all 3 steps: step 0 OK, step 1 fails but skips, step 2 OK.
    for i in 0..3 {
        gateway.advance_chain("test", "tenant-1", &chain_id).await?;
        println!("  Step {i} advanced");
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let chain_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");
    println!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );

    println!("  Provider call counts:");
    println!("    step-a: {}", ok_provider.call_count());
    println!(
        "    step-b: {} (failed, skipped)",
        fail_provider.call_count()
    );
    println!(
        "    step-c: {} (still reached)",
        final_provider.call_count()
    );
    assert_eq!(
        final_provider.call_count(),
        1,
        "step-c should be reached after skip"
    );

    let page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id.clone()),
            ..Default::default()
        })
        .await?;
    println!("\n  Audit records for chain: {}", page.total);
    for rec in &page.records {
        println!(
            "    [{:>22}] provider={:<10} action_type={}",
            rec.outcome, rec.provider, rec.action_type
        );
    }

    let skipped: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.outcome == "chain_step_skipped")
        .collect();
    assert_eq!(skipped.len(), 1, "expected 1 chain_step_skipped record");

    let terminal: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.outcome == "chain_completed")
        .collect();
    assert_eq!(
        terminal.len(),
        1,
        "chain should complete despite skipped step"
    );

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 4: Chain Cancellation
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 4: CHAIN CANCELLATION");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    // Use a provider with a delay to simulate slow steps.
    let slow_provider = Arc::new(RecordingProvider::new("slow-svc"));
    let webhook_provider = Arc::new(RecordingProvider::new("webhook"));

    let chain_config = ChainConfig::new("cancellable-chain")
        .with_step(ChainStepConfig::new(
            "step-1",
            "slow-svc",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "step-2",
            "slow-svc",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "step-3",
            "slow-svc",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(120);

    let cancel_rule: &str = r#"
rules:
  - name: trigger-cancel-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "trigger_cancel"
    action:
      type: chain
      chain: cancellable-chain
"#;
    let rules = parse_rules(cancel_rule);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&slow_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&webhook_provider) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "slow-svc",
        "trigger_cancel",
        serde_json::json!({}),
    );

    println!("  Dispatching action to trigger cancellable-chain...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => {
            println!("  Unexpected outcome: {other:?}");
            return Ok(());
        }
    };

    // Advance step 0 so chain is partially complete.
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 0 advanced");

    // Cancel the chain while step 1 is pending.
    println!("  Cancelling chain...");
    let cancelled_state = gateway
        .cancel_chain(
            "test",
            "tenant-1",
            &chain_id,
            Some("user requested cancellation".into()),
            Some("admin@example.com".into()),
        )
        .await?;

    println!("  Chain status after cancel: {:?}", cancelled_state.status);
    println!("  Cancel reason: {:?}", cancelled_state.cancel_reason);
    println!("  Cancelled by: {:?}", cancelled_state.cancelled_by);
    assert_eq!(
        cancelled_state.status,
        acteon_core::chain::ChainStatus::Cancelled
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    let page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id.clone()),
            ..Default::default()
        })
        .await?;
    println!("\n  Audit records for chain: {}", page.total);
    for rec in &page.records {
        println!(
            "    [{:>22}] provider={:<10} action_type={}",
            rec.outcome, rec.provider, rec.action_type
        );
    }

    let cancelled: Vec<_> = page
        .records
        .iter()
        .filter(|r| r.outcome == "chain_cancelled")
        .collect();
    assert_eq!(
        cancelled.len(),
        1,
        "expected 1 chain_cancelled terminal record"
    );

    // Verify terminal record has cancel details.
    let terminal_rec = cancelled[0];
    let details = &terminal_rec.outcome_details;
    assert_eq!(
        details["cancel_reason"].as_str(),
        Some("user requested cancellation")
    );
    assert_eq!(details["cancelled_by"].as_str(), Some("admin@example.com"));

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 5: Audit Trail Filter by chain_id
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 5: AUDIT TRAIL QUERIES");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let provider_a = Arc::new(RecordingProvider::new("svc-a"));
    let provider_b = Arc::new(RecordingProvider::new("svc-b"));

    let chain_a = ChainConfig::new("chain-alpha")
        .with_step(ChainStepConfig::new(
            "step1",
            "svc-a",
            "task",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "step2",
            "svc-a",
            "task",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let chain_b = ChainConfig::new("chain-beta")
        .with_step(ChainStepConfig::new(
            "step1",
            "svc-b",
            "task",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let multi_rule: &str = r#"
rules:
  - name: trigger-alpha
    priority: 1
    condition:
      field: action.action_type
      eq: "alpha"
    action:
      type: chain
      chain: chain-alpha
  - name: trigger-beta
    priority: 1
    condition:
      field: action.action_type
      eq: "beta"
    action:
      type: chain
      chain: chain-beta
"#;
    let rules = parse_rules(multi_rule);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&provider_a) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&provider_b) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .audit_store_payload(true)
        .chain(chain_a)
        .chain(chain_b)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    // Dispatch chain-alpha.
    let action_a = Action::new("test", "tenant-1", "svc-a", "alpha", serde_json::json!({}));
    let outcome_a = gateway.dispatch(action_a, None).await?;
    let chain_id_a = match &outcome_a {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Dispatch chain-beta.
    let action_b = Action::new("test", "tenant-1", "svc-b", "beta", serde_json::json!({}));
    let outcome_b = gateway.dispatch(action_b, None).await?;
    let chain_id_b = match &outcome_b {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Advance both chains.
    for _ in 0..2 {
        gateway
            .advance_chain("test", "tenant-1", &chain_id_a)
            .await?;
    }
    gateway
        .advance_chain("test", "tenant-1", &chain_id_b)
        .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query all audit records.
    let all_page = audit.query(&AuditQuery::default()).await?;
    println!("  Total audit records (all chains): {}", all_page.total);

    // Query only chain-alpha records.
    let alpha_page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id_a.clone()),
            ..Default::default()
        })
        .await?;
    println!(
        "  Records for chain-alpha ({}): {}",
        chain_id_a, alpha_page.total
    );
    for rec in &alpha_page.records {
        println!(
            "    [{:>22}] provider={:<10} action_type={}",
            rec.outcome, rec.provider, rec.action_type
        );
    }

    // Query only chain-beta records.
    let beta_page = audit
        .query(&AuditQuery {
            chain_id: Some(chain_id_b.clone()),
            ..Default::default()
        })
        .await?;
    println!(
        "  Records for chain-beta  ({}): {}",
        chain_id_b, beta_page.total
    );
    for rec in &beta_page.records {
        println!(
            "    [{:>22}] provider={:<10} action_type={}",
            rec.outcome, rec.provider, rec.action_type
        );
    }

    // chain-alpha: 1 dispatch + 2 steps + 1 terminal = 4 records.
    // chain-beta:  1 dispatch + 1 step  + 1 terminal = 3 records.
    assert_eq!(
        alpha_page.total, 4,
        "chain-alpha: 1 dispatch + 2 step + 1 terminal"
    );
    assert_eq!(
        beta_page.total, 3,
        "chain-beta: 1 dispatch + 1 step + 1 terminal"
    );

    // Verify no cross-contamination.
    for rec in &alpha_page.records {
        assert_eq!(rec.chain_id.as_deref(), Some(chain_id_a.as_str()));
    }
    for rec in &beta_page.records {
        assert_eq!(rec.chain_id.as_deref(), Some(chain_id_b.as_str()));
    }

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    println!("==================================================================");
    println!("           ALL CHAIN SIMULATIONS PASSED");
    println!("==================================================================");

    Ok(())
}
