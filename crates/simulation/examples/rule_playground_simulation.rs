//! Demonstration of the Rule Playground (evaluate-without-dispatch) feature.
//!
//! Run with: cargo run -p acteon-simulation --example rule_playground_simulation

use std::collections::HashMap;

use acteon_core::Action;
use acteon_simulation::prelude::*;

const RULES: &str = r#"
rules:
  - name: block-spam
    priority: 1
    description: Block spam actions
    condition:
      field: action.payload.category
      eq: "spam"
    action:
      type: suppress

  - name: reroute-urgent
    priority: 5
    description: Reroute urgent alerts to SMS
    condition:
      field: action.payload.priority
      eq: "urgent"
    action:
      type: reroute
      target_provider: sms

  - name: enrich-payload
    priority: 10
    description: Add default sender to emails
    condition:
      field: action.action_type
      eq: "email"
    action:
      type: modify
      changes:
        from: "noreply@acme.com"
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           RULE PLAYGROUND SIMULATION DEMO                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(RULES)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster with 1 node");
    println!("✓ Loaded 3 rules: block-spam, reroute-urgent, enrich-payload\n");

    // =========================================================================
    // DEMO 1: Basic evaluation trace
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 1: BASIC EVALUATION (spam action)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let spam = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "alert",
        serde_json::json!({ "category": "spam", "body": "Buy now!" }),
    );

    let trace = harness
        .node(0)
        .unwrap()
        .gateway()
        .evaluate_rules(&spam, false, false, None, HashMap::new())
        .await?;

    println!("  Verdict: {}", trace.verdict);
    println!("  Matched rule: {:?}", trace.matched_rule);
    println!("  Duration: {}µs", trace.evaluation_duration_us);
    println!("  Rules evaluated: {}", trace.total_rules_evaluated);
    println!("  Rules skipped: {}", trace.total_rules_skipped);
    println!();
    for entry in &trace.trace {
        println!(
            "  [{:>12}] {} (priority={}, result={})",
            entry.source,
            entry.rule_name,
            entry.priority,
            entry.result.as_str()
        );
    }
    println!(
        "\n  ✓ No side effects: provider call count = {}\n",
        harness.provider("email").unwrap().call_count()
    );

    // =========================================================================
    // DEMO 2: Evaluate-all mode
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 2: EVALUATE ALL RULES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let trace_all = harness
        .node(0)
        .unwrap()
        .gateway()
        .evaluate_rules(&spam, false, true, None, HashMap::new())
        .await?;

    println!("  Verdict: {}", trace_all.verdict);
    println!("  Evaluate-all: every rule condition was checked");
    for entry in &trace_all.trace {
        println!("    {} -> {}", entry.rule_name, entry.result.as_str());
    }

    // =========================================================================
    // DEMO 3: Modify payload preview
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 3: MODIFY PAYLOAD PREVIEW");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let email = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "email",
        serde_json::json!({ "to": "user@example.com", "subject": "Hello" }),
    );

    let trace_modify = harness
        .node(0)
        .unwrap()
        .gateway()
        .evaluate_rules(&email, false, false, None, HashMap::new())
        .await?;

    println!("  Verdict: {}", trace_modify.verdict);
    println!("  Matched: {:?}", trace_modify.matched_rule);
    if let Some(ref payload) = trace_modify.modified_payload {
        println!(
            "  Modified payload: {}",
            serde_json::to_string_pretty(payload)?
        );
    }

    // =========================================================================
    // DEMO 4: Default fallthrough (no match)
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 4: DEFAULT FALLTHROUGH (no rules match)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harmless = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "order_shipped",
        serde_json::json!({ "order_id": "ORD-123" }),
    );

    let trace_allow = harness
        .node(0)
        .unwrap()
        .gateway()
        .evaluate_rules(&harmless, false, false, None, HashMap::new())
        .await?;

    println!("  Verdict: {}", trace_allow.verdict);
    println!("  Matched rule: {:?}", trace_allow.matched_rule);
    // The last entry should be the synthetic default-fallthrough
    if let Some(last) = trace_allow.trace.last() {
        println!(
            "  Last trace entry: {} (result={})",
            last.rule_name,
            last.result.as_str()
        );
    }

    println!(
        "\n  ✓ Provider was never called: {}",
        harness.provider("email").unwrap().call_count() == 0
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down");

    Ok(())
}
