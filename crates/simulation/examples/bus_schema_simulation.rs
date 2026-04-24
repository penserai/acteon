//! Phase 3 schema-registry + publish-edge validation demo.
//!
//! Runs entirely against the in-memory bus backend plus a compiled
//! [`SchemaValidator`], so no Kafka or HTTP server is required. The
//! same validator lives behind `/v1/bus/publish` in production; this
//! example exercises the shared code path end-to-end.
//!
//! Scenarios:
//!
//! 1. Register `orders` v1.
//! 2. Validate a conforming payload — accepted.
//! 3. Validate a non-conforming payload — rejected with per-field
//!    JSON-Pointer paths.
//! 4. Register a stricter `orders` v2 that requires a new `sku` field.
//! 5. Re-validate the v1 payload against v2 — rejected; confirm v1
//!    still accepts it (versions are independent).
//! 6. Remove v1 from the validator cache (simulating a deletion after
//!    unbinding) — further v1 validations return `NotFound`.
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example bus_schema_simulation
//! ```

use acteon_bus::{SchemaValidator, SchemaValidatorError};
use serde_json::json;
use tracing::{Level, info, warn};

fn orders_v1() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["id", "qty"],
        "properties": {
            "id": {"type": "string"},
            "qty": {"type": "integer", "minimum": 1}
        }
    })
}

fn orders_v2() -> serde_json::Value {
    json!({
        "type": "object",
        "required": ["id", "qty", "sku"],
        "properties": {
            "id": {"type": "string"},
            "qty": {"type": "integer", "minimum": 1},
            "sku": {"type": "string", "pattern": "^[A-Z]{3}-[0-9]+$"}
        }
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .init();

    let validator = SchemaValidator::new();

    // 1. Register orders v1.
    info!("registering schema orders v1");
    validator.register("agents", "demo", "orders", 1, &orders_v1())?;

    // 2. Valid payload.
    let good = json!({"id": "ord-1", "qty": 2});
    info!(payload = %good, "validating conforming payload");
    validator.validate("agents", "demo", "orders", 1, &good)?;
    info!("v1 accepted the payload");

    // 3. Invalid payload — missing required field.
    let bad = json!({"id": "ord-2"});
    info!(payload = %bad, "validating non-conforming payload");
    match validator.validate("agents", "demo", "orders", 1, &bad) {
        Err(SchemaValidatorError::InvalidPayload(issues)) => {
            warn!(count = issues.len(), "payload rejected");
            for issue in &issues {
                warn!(path = %issue.path, message = %issue.message, "schema issue");
            }
        }
        other => return Err(format!("expected InvalidPayload, got {other:?}").into()),
    }

    // 4. Register stricter v2.
    info!("registering schema orders v2 (adds sku)");
    validator.register("agents", "demo", "orders", 2, &orders_v2())?;

    // 5. Versions are independent.
    let without_sku = json!({"id": "ord-3", "qty": 1});
    match validator.validate("agents", "demo", "orders", 2, &without_sku) {
        Err(e) => warn!(error = %e, "v2 rejects payloads that v1 accepts"),
        Ok(()) => return Err("v2 should have rejected payload missing sku".into()),
    }
    validator.validate("agents", "demo", "orders", 1, &without_sku)?;
    info!("v1 still accepts the same payload — versions are independent");

    let full = json!({"id": "ord-4", "qty": 3, "sku": "ABC-42"});
    validator.validate("agents", "demo", "orders", 2, &full)?;
    info!("v2 accepts payloads with a well-formed sku");

    // 6. Remove v1 — subsequent validations return NotFound.
    validator.remove("agents", "demo", "orders", 1);
    match validator.validate("agents", "demo", "orders", 1, &good) {
        Err(SchemaValidatorError::NotFound { .. }) => {
            info!(
                "v1 no longer cached after remove(); the server layer recompiles on next publish"
            );
        }
        other => return Err(format!("expected NotFound, got {other:?}").into()),
    }

    info!("schema simulation complete");
    Ok(())
}
