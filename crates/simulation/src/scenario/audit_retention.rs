//! Deterministic audit-record retention evidence over the production memory store.

use std::sync::Arc;
use std::time::Duration;

use acteon_audit::{AuditQuery, AuditRecord, AuditStore};
use acteon_audit_memory::MemoryAuditStore;
use acteon_time::{Clock, ManualClock};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::{Value, json};

use super::{Backend, Scenario, ScenarioReport};
use crate::SimulationError;

const SCENARIO: Scenario = Scenario::AuditRetentionRecovery;
const EPOCH: i64 = 1_700_000_000;

fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Configuration(error.to_string())
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        DateTime::from_timestamp(EPOCH, 0).expect("fixed epoch"),
    ))
}

fn record(
    id: &str,
    action_id: &str,
    dispatched_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
) -> AuditRecord {
    AuditRecord {
        id: id.to_owned(),
        action_id: action_id.to_owned(),
        chain_id: None,
        namespace: "retention".to_owned(),
        tenant: "alice".to_owned(),
        provider: "audit-fixture".to_owned(),
        action_type: "retention_probe".to_owned(),
        verdict: "allow".to_owned(),
        matched_rule: None,
        outcome: "executed".to_owned(),
        action_payload: None,
        verdict_details: json!({}),
        outcome_details: json!({}),
        metadata: json!({}),
        dispatched_at,
        completed_at: dispatched_at,
        duration_ms: 0,
        expires_at,
        caller_id: String::new(),
        auth_method: String::new(),
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        kid: None,
        canonical_hash: None,
    }
}

fn check(report: &mut ScenarioReport, name: &str, passed: bool, evidence: &Value) {
    let detail = evidence.to_string();
    report.check(SCENARIO, name, passed, &detail);
    report.event(SCENARIO, name, &detail, 0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    SkipAdvance,
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}

async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    if report.manifest.backend != Backend::Memory {
        return Err(error(
            "audit_retention_recovery requires the memory backend; remote audit clocks are not virtualized",
        ));
    }

    let clock = clock();
    let store = Arc::new(MemoryAuditStore::with_clock(clock.clone()));
    let epoch = clock.now();
    let expiry = epoch + ChronoDuration::seconds(10);
    let future_expiry = epoch + ChronoDuration::seconds(20);
    store
        .record(record("expired", "shared-action", epoch, Some(expiry)))
        .await
        .map_err(error)?;
    store
        .record(record(
            "future",
            "shared-action",
            epoch + ChronoDuration::seconds(1),
            Some(future_expiry),
        ))
        .await
        .map_err(error)?;
    store
        .record(record("permanent", "permanent-action", epoch, None))
        .await
        .map_err(error)?;

    clock.advance_to(Duration::from_secs(9)).map_err(error)?;
    let before_removed = store.cleanup_expired().await.map_err(error)?;
    let before = store.get_by_id("expired").await.map_err(error)?.is_some();

    let should_advance = mutation != Mutation::SkipAdvance;
    if should_advance {
        clock.advance_to(Duration::from_secs(10)).map_err(error)?;
    }
    let removed = store.cleanup_expired().await.map_err(error)?;
    let expired_missing = store.get_by_id("expired").await.map_err(error)?.is_none();
    let future_present = store.get_by_id("future").await.map_err(error)?.is_some();
    let latest_shared = store
        .get_by_action_id("shared-action")
        .await
        .map_err(error)?
        .is_some_and(|entry| entry.id == "future");
    let total = store
        .query(&AuditQuery::default())
        .await
        .map_err(error)?
        .total
        .unwrap_or_default();
    let repeated = store.cleanup_expired().await.map_err(error)?;

    check(
        report,
        "expiry_boundary",
        before_removed == 0 && before && removed == 1 && expired_missing && repeated == 0,
        &json!({
            "before_removed": before_removed,
            "removed_at_expiry": removed,
            "expired_missing": expired_missing,
            "repeated": repeated,
        }),
    );
    check(
        report,
        "index_consistency",
        future_present && latest_shared && total == 2,
        &json!({
            "future_present": future_present,
            "latest_shared": latest_shared,
            "remaining": total,
        }),
    );
    check(
        report,
        "manual_clock",
        clock.now() == epoch + ChronoDuration::seconds(10),
        &json!({
            "elapsed_seconds": clock.monotonic().as_secs(),
            "now": clock.now(),
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;

    #[tokio::test]
    async fn retention_replays_and_missing_advance_fails_the_gate() {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 20_260_912,
            backend: Backend::Memory,
            scenarios: vec![SCENARIO],
        };
        let baseline = super::super::run(manifest.clone()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        assert!(
            super::super::run(manifest.clone())
                .await
                .unwrap()
                .same_evidence(&baseline)
        );
        let mut mutated = super::super::run(manifest).await.unwrap();
        mutated.invariants.clear();
        mutated.trace.clear();
        run_with(&mut mutated, Mutation::SkipAdvance).await.unwrap();
        assert!(
            mutated
                .invariants
                .iter()
                .any(|check| check.name == "expiry_boundary" && !check.passed)
        );
        assert!(!evaluation::grade(SCENARIO, &mutated).passed);
    }
}
