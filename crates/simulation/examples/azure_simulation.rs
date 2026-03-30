//! Simulation demonstrating Azure Blob Storage and Event Hubs provider action types.
//!
//! Covers all Blob Storage operations (upload, upload base64, download, delete) and
//! Event Hubs operations (send event, send event with partition, send batch).
//!
//! Run with:
//! ```bash
//! cargo run -p acteon-simulation --example azure_simulation
//! ```

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("==================================================================");
    info!("       AZURE BLOB STORAGE & EVENT HUBS SIMULATION");
    info!("==================================================================\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("azure-blob")
            .add_recording_provider("azure-eventhubs")
            .build(),
    )
    .await?;

    info!("Started simulation cluster with 1 node");
    info!("Registered providers: azure-blob, azure-eventhubs\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // 1. Blob: Upload (text body)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  1. BLOB: UPLOAD (text body)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "azure-blob",
        "upload_blob",
        serde_json::json!({
            "container": "data-lake",
            "blob_name": "reports/2026/02/daily.json",
            "body": "{\"readings\": [72.5, 68.1, 75.3]}",
            "content_type": "application/json",
            "metadata": {
                "source": "acteon",
                "pipeline_version": "2.0"
            }
        }),
    );

    info!("  Dispatching upload_blob with text body, content_type, and metadata...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("blob/upload_blob(text)", outcome));
    info!("");

    // =========================================================================
    // 2. Blob: Upload (base64 body)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  2. BLOB: UPLOAD (base64 body)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "azure-blob",
        "upload_blob",
        serde_json::json!({
            "container": "data-lake",
            "blob_name": "images/logo.png",
            "body_base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk",
            "content_type": "image/png"
        }),
    );

    info!("  Dispatching upload_blob with base64-encoded binary body...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("blob/upload_blob(base64)", outcome));
    info!("");

    // =========================================================================
    // 3. Blob: Download
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  3. BLOB: DOWNLOAD");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "azure-blob",
        "download_blob",
        serde_json::json!({
            "container": "data-lake",
            "blob_name": "reports/2026/02/daily.json"
        }),
    );

    info!("  Dispatching download_blob...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("blob/download_blob", outcome));
    info!("");

    // =========================================================================
    // 4. Blob: Delete
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  4. BLOB: DELETE");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "storage",
        "acme-corp",
        "azure-blob",
        "delete_blob",
        serde_json::json!({
            "container": "data-lake",
            "blob_name": "old/data.csv"
        }),
    );

    info!("  Dispatching delete_blob...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("blob/delete_blob", outcome));
    info!("");

    // =========================================================================
    // 5. Event Hubs: Send Event (JSON body)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  5. EVENT HUBS: SEND EVENT (JSON body)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "events",
        "acme-corp",
        "azure-eventhubs",
        "send_event",
        serde_json::json!({
            "body": {
                "device_id": "sensor-001",
                "temperature": 72.5,
                "unit": "F"
            },
            "properties": {
                "source": "iot-gateway",
                "region": "us-west"
            }
        }),
    );

    info!("  Dispatching send_event with JSON body and application properties...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("eventhubs/send_event(json)", outcome));
    info!("");

    // =========================================================================
    // 6. Event Hubs: Send Event (with partition)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  6. EVENT HUBS: SEND EVENT (with partition)");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "events",
        "acme-corp",
        "azure-eventhubs",
        "send_event",
        serde_json::json!({
            "event_hub_name": "telemetry",
            "body": {
                "device_id": "sensor-042",
                "temperature": 68.1,
                "unit": "F"
            },
            "partition_id": "0"
        }),
    );

    info!("  Dispatching send_event with explicit partition_id and event_hub_name override...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("eventhubs/send_event(partition)", outcome));
    info!("");

    // =========================================================================
    // 7. Event Hubs: Send Batch
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  7. EVENT HUBS: SEND BATCH");
    info!("------------------------------------------------------------------\n");

    let action = Action::new(
        "events",
        "acme-corp",
        "azure-eventhubs",
        "send_batch",
        serde_json::json!({
            "event_hub_name": "telemetry",
            "events": [
                {
                    "body": {"device_id": "sensor-001", "temperature": 72.5},
                    "properties": {"region": "us-west"}
                },
                {
                    "body": {"device_id": "sensor-002", "temperature": 68.1},
                    "partition_id": "1"
                },
                {
                    "body": {"device_id": "sensor-003", "temperature": 75.3}
                }
            ]
        }),
    );

    info!("  Dispatching send_batch with 3 events (mixed properties/partitions)...");
    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    results.push(("eventhubs/send_batch", outcome));
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
        "  Total dispatched: {}  |  Blob calls: {}  |  Event Hubs calls: {}",
        results.len(),
        harness.provider("azure-blob").unwrap().call_count(),
        harness.provider("azure-eventhubs").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("\n  Simulation cluster shut down");

    if all_passed {
        info!("\n  All Azure Blob Storage and Event Hubs actions dispatched successfully.");
    } else {
        info!("\n  Some actions failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
