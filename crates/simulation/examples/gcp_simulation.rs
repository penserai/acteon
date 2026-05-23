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
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("==================================================================");
    info!("       GCP CLOUD STORAGE & PUB/SUB SIMULATION");
    info!("==================================================================\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("gcp-storage")
            .add_recording_provider("gcp-pubsub")
            .build(),
    )
    .await?;

    info!("Started simulation cluster with 1 node");
    info!("Registered providers: gcp-storage, gcp-pubsub\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // 1. Storage: Upload (text body)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  1. STORAGE: UPLOAD (text body)");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching upload_object with text body, content_type, and metadata...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("storage/upload_object(text)", outcome));
    info!("");

    // =========================================================================
    // 2. Storage: Upload (base64 body)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  2. STORAGE: UPLOAD (base64 body)");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching upload_object with base64-encoded binary body...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("storage/upload_object(base64)", outcome));
    info!("");

    // =========================================================================
    // 3. Storage: Download
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  3. STORAGE: DOWNLOAD");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching download_object...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("storage/download_object", outcome));
    info!("");

    // =========================================================================
    // 4. Storage: Delete
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  4. STORAGE: DELETE");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching delete_object...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("storage/delete_object", outcome));
    info!("");

    // =========================================================================
    // 5. Pub/Sub: Publish (JSON data + attributes)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  5. PUB/SUB: PUBLISH (JSON data + attributes)");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching publish with JSON data string and attributes...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("pubsub/publish(json+attrs)", outcome));
    info!("");

    // =========================================================================
    // 6. Pub/Sub: Publish (with ordering key)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  6. PUB/SUB: PUBLISH (with ordering key)");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching publish with ordering_key and topic override...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("pubsub/publish(ordering_key)", outcome));
    info!("");

    // =========================================================================
    // 7. Pub/Sub: Publish Batch
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  7. PUB/SUB: PUBLISH BATCH");
    info!("------------------------------------------------------------------\n");

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

    info!("  Dispatching publish_batch with 3 messages (mixed attributes/ordering keys)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("pubsub/publish_batch", outcome));
    info!("");

    // =========================================================================
    // Summary
    // =========================================================================
    info!("==================================================================");
    info!("  SUMMARY");
    info!("==================================================================\n");

    let mut all_passed = true;
    for (name, outcome) in &results {
        let passed = matches!(outcome, ActionOutcome::Executed(_));
        let status = if passed { "PASS" } else { "FAIL" };
        info!("  [{status}] {name}: {outcome:?}");
        if !passed {
            all_passed = false;
        }
    }

    info!("");
    info!(
        "  Total dispatched: {}  |  Storage calls: {}  |  Pub/Sub calls: {}",
        results.len(),
        harness.provider("gcp-storage").unwrap().call_count(),
        harness.provider("gcp-pubsub").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("\n  Simulation cluster shut down");

    if all_passed {
        info!("\n  All GCP Cloud Storage and Pub/Sub actions dispatched successfully.");
    } else {
        info!("\n  Some actions failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
