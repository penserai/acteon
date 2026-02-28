//! Simulation demonstrating GCP Cloud Storage and Pub/Sub provider action types.
//!
//! Covers all Cloud Storage operations (upload, upload base64, download, delete) and
//! Pub/Sub operations (publish, publish with ordering key, publish batch).
//!
//! Run with:
//! ```bash
//! cargo run -p acteon-simulation --example gcp_simulation
//! ```

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("       GCP CLOUD STORAGE & PUB/SUB SIMULATION");
    println!("==================================================================\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("gcp-storage")
            .add_recording_provider("gcp-pubsub")
            .build(),
    )
    .await?;

    println!("Started simulation cluster with 1 node");
    println!("Registered providers: gcp-storage, gcp-pubsub\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // 1. Storage: Upload (text body)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  1. STORAGE: UPLOAD (text body)");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "gcp-storage",
        "upload_object",
        serde_json::json!({
            "bucket": "data-lake",
            "object_name": "reports/2026/02/daily.json",
            "body": "{\"readings\": [72.5, 68.1, 75.3]}",
            "content_type": "application/json",
            "metadata": {
                "source": "acteon",
                "pipeline_version": "2.0"
            }
        }),
    );

    println!("  Dispatching upload_object with text body, content_type, and metadata...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("storage/upload_object(text)", outcome));
    println!();

    // =========================================================================
    // 2. Storage: Upload (base64 body)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  2. STORAGE: UPLOAD (base64 body)");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "gcp-storage",
        "upload_object",
        serde_json::json!({
            "bucket": "data-lake",
            "object_name": "images/logo.png",
            "body_base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk",
            "content_type": "image/png"
        }),
    );

    println!("  Dispatching upload_object with base64-encoded binary body...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("storage/upload_object(base64)", outcome));
    println!();

    // =========================================================================
    // 3. Storage: Download
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  3. STORAGE: DOWNLOAD");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "gcp-storage",
        "download_object",
        serde_json::json!({
            "bucket": "data-lake",
            "object_name": "reports/2026/02/daily.json"
        }),
    );

    println!("  Dispatching download_object...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("storage/download_object", outcome));
    println!();

    // =========================================================================
    // 4. Storage: Delete
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  4. STORAGE: DELETE");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "gcp-storage",
        "delete_object",
        serde_json::json!({
            "bucket": "data-lake",
            "object_name": "old/data.csv"
        }),
    );

    println!("  Dispatching delete_object...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("storage/delete_object", outcome));
    println!();

    // =========================================================================
    // 5. Pub/Sub: Publish (JSON data + attributes)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  5. PUB/SUB: PUBLISH (JSON data + attributes)");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "events",
        "acme-corp",
        "gcp-pubsub",
        "publish",
        serde_json::json!({
            "data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5, \"unit\": \"F\"}",
            "attributes": {
                "source": "iot-gateway",
                "region": "us-west1"
            }
        }),
    );

    println!("  Dispatching publish with JSON data string and attributes...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("pubsub/publish(json+attrs)", outcome));
    println!();

    // =========================================================================
    // 6. Pub/Sub: Publish (with ordering key)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  6. PUB/SUB: PUBLISH (with ordering key)");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "events",
        "acme-corp",
        "gcp-pubsub",
        "publish",
        serde_json::json!({
            "data": "{\"device_id\": \"sensor-042\", \"temperature\": 68.1, \"unit\": \"F\"}",
            "ordering_key": "sensor-042",
            "topic": "telemetry"
        }),
    );

    println!("  Dispatching publish with ordering_key and topic override...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("pubsub/publish(ordering_key)", outcome));
    println!();

    // =========================================================================
    // 7. Pub/Sub: Publish Batch
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  7. PUB/SUB: PUBLISH BATCH");
    println!("------------------------------------------------------------------\n");

    let action = Action::new(
        "events",
        "acme-corp",
        "gcp-pubsub",
        "publish_batch",
        serde_json::json!({
            "topic": "telemetry",
            "messages": [
                {
                    "data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}",
                    "attributes": {"region": "us-west1"}
                },
                {
                    "data": "{\"device_id\": \"sensor-002\", \"temperature\": 68.1}",
                    "ordering_key": "sensor-002"
                },
                {
                    "data": "{\"device_id\": \"sensor-003\", \"temperature\": 75.3}"
                }
            ]
        }),
    );

    println!("  Dispatching publish_batch with 3 messages (mixed attributes/ordering keys)...");
    let outcome = harness.dispatch(&action).await?;
    println!("  Outcome: {outcome:?}");
    results.push(("pubsub/publish_batch", outcome));
    println!();

    // =========================================================================
    // Summary
    // =========================================================================
    println!("==================================================================");
    println!("  SUMMARY");
    println!("==================================================================\n");

    let mut all_passed = true;
    for (name, outcome) in &results {
        let passed = matches!(outcome, ActionOutcome::Executed(_));
        let status = if passed { "PASS" } else { "FAIL" };
        println!("  [{status}] {name}: {outcome:?}");
        if !passed {
            all_passed = false;
        }
    }

    println!();
    println!(
        "  Total dispatched: {}  |  Storage calls: {}  |  Pub/Sub calls: {}",
        results.len(),
        harness.provider("gcp-storage").unwrap().call_count(),
        harness.provider("gcp-pubsub").unwrap().call_count(),
    );

    harness.teardown().await?;
    println!("\n  Simulation cluster shut down");

    if all_passed {
        println!("\n  All GCP Cloud Storage and Pub/Sub actions dispatched successfully.");
    } else {
        println!("\n  Some actions failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
