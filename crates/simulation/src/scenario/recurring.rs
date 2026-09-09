//! Executable recovery evidence for the recurring-dispatch lease protocol.
//!
//! `RecurringDispatch.tla` models two workers sharing an occurrence claim. This
//! scenario drives the same production worker tick against a shared manual-clock
//! state store: worker A emits an occurrence, its claim lease expires before the
//! consumer acknowledges it, and worker B polls. The re-armed pending index must
//! keep B from emitting that already handed-off occurrence a second time.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acteon_core::{OverlapPolicy, RecurringAction, RecurringActionTemplate};
use acteon_gateway::{
    BackgroundConfig, BackgroundJob, BackgroundProcessor, BackgroundProcessorBuilder,
    GatewayMetrics, GroupManager,
};
use acteon_state::{KeyKind, StateKey, StateStore, set_pending_recurring};
use acteon_state_memory::MemoryStateStore;
use acteon_time::{Clock, ManualClock};
use serde_json::json;
use tokio::sync::mpsc;

use super::{Scenario, ScenarioReport};
use crate::SimulationError;

const SCENARIO: Scenario = Scenario::RecurringDispatchRecovery;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const RECOVERY_MARGIN: Duration = Duration::from_secs(30);
const RECURRING_ID: &str = "recurring-lease-recovery";

fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(error.to_string())
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("fixed epoch"),
    ))
}

fn record(report: &mut ScenarioReport, name: &str, passed: bool, evidence: &serde_json::Value) {
    let detail = evidence.to_string();
    report.check(SCENARIO, name, passed, &detail);
    report.event(SCENARIO, name, &detail, 0);
}

fn recurring_action(now: chrono::DateTime<chrono::Utc>) -> RecurringAction {
    RecurringAction {
        id: RECURRING_ID.into(),
        namespace: "recurring-recovery".into(),
        tenant: "alice".into(),
        // The next occurrence after the fixed epoch is five minutes away,
        // comfortably beyond the claim lease used in this scenario.
        cron_expr: "*/5 * * * *".into(),
        timezone: "UTC".into(),
        enabled: true,
        action_template: RecurringActionTemplate {
            provider: "effect".into(),
            action_type: "notify".into(),
            payload: json!({"source":"recurring-dispatch-recovery"}),
            metadata: HashMap::new(),
            dedup_key: Some("{{recurring_id}}:{{execution_time}}".into()),
        },
        created_at: now,
        updated_at: now,
        last_executed_at: None,
        next_execution_at: Some(now),
        ends_at: None,
        max_executions: None,
        execution_count: 0,
        description: None,
        labels: HashMap::new(),
        overlap_policy: OverlapPolicy::AllowAll,
        last_execution_id: None,
    }
}

fn worker(
    clock: &Arc<ManualClock>,
    state: &Arc<MemoryStateStore>,
    tx: mpsc::Sender<acteon_gateway::background::RecurringActionDueEvent>,
) -> Result<BackgroundProcessor, SimulationError> {
    let (worker, _shutdown) = BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(state.clone())
        .group_manager(Arc::new(GroupManager::with_clock(clock.clone())))
        .metrics(Arc::new(GatewayMetrics::default()))
        .recurring_action_channel(tx)
        .config(BackgroundConfig {
            enable_recurring_actions: true,
            recurring_check_interval: POLL_INTERVAL,
            ..Default::default()
        })
        .build()
        .map_err(error)?;
    Ok(worker)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    RestoreStaleDueIndex,
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}

async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let clock = clock();
    let state = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let now = clock.now();
    let action = recurring_action(now);
    let recurring_key = StateKey::new(
        action.namespace.as_str(),
        action.tenant.as_str(),
        KeyKind::RecurringAction,
        RECURRING_ID,
    );
    let pending_key = StateKey::new(
        action.namespace.as_str(),
        action.tenant.as_str(),
        KeyKind::PendingRecurring,
        RECURRING_ID,
    );
    state
        .set(
            &recurring_key,
            &serde_json::to_string(&action).map_err(error)?,
            None,
        )
        .await
        .map_err(error)?;
    set_pending_recurring(state.as_ref(), &pending_key, now.timestamp_millis())
        .await
        .map_err(error)?;

    let (tx, mut events) = mpsc::channel(4);
    let mut worker_a = worker(&clock, &state, tx.clone())?;
    let mut worker_b = worker(&clock, &state, tx)?;
    let claim_ttl = BackgroundConfig {
        enable_recurring_actions: true,
        recurring_check_interval: POLL_INTERVAL,
        ..Default::default()
    }
    .recurring_claim_ttl();

    worker_a
        .tick(BackgroundJob::RecurringActions)
        .await
        .map_err(error)?;
    let first = events.try_recv().ok();
    let armed_ms = state
        .get(&pending_key)
        .await
        .map_err(error)?
        .and_then(|value| value.parse::<i64>().ok());
    record(
        report,
        "next_occurrence_armed",
        first
            .as_ref()
            .is_some_and(|event| event.recurring_id == RECURRING_ID)
            && armed_ms.is_some_and(|armed| armed > now.timestamp_millis()),
        &json!({
            "first_event": first.as_ref().map(|event| event.recurring_id.as_str()),
            "due_at_ms": now.timestamp_millis(),
            "armed_at_ms": armed_ms,
        }),
    );

    if mutation == Mutation::RestoreStaleDueIndex {
        // This is the pre-fix state: the occurrence remains due while the
        // consumer is still unavailable. The expired lease then permits a
        // second worker to hand off the same occurrence.
        set_pending_recurring(state.as_ref(), &pending_key, now.timestamp_millis())
            .await
            .map_err(error)?;
    }
    clock.advance_to(claim_ttl).map_err(error)?;
    worker_b
        .tick(BackgroundJob::RecurringActions)
        .await
        .map_err(error)?;
    let redeliveries: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    record(
        report,
        "lease_expiry_no_redelivery",
        first.is_some() && redeliveries.is_empty(),
        &json!({
            "claim_ttl_ms": claim_ttl.as_millis(),
            "lease_expired_at_ms": clock.now().timestamp_millis(),
            "redelivery_count": redeliveries.len(),
            "redelivery_ids": redeliveries.iter().map(|event| event.recurring_id.as_str()).collect::<Vec<_>>(),
        }),
    );
    let expected_ttl = POLL_INTERVAL
        .checked_mul(2)
        .expect("fixed polling interval")
        .saturating_add(RECOVERY_MARGIN);
    record(
        report,
        "lease_covers_polling_windows",
        claim_ttl >= expected_ttl,
        &json!({
            "poll_interval_ms": POLL_INTERVAL.as_millis(),
            "recovery_margin_ms": RECOVERY_MARGIN.as_millis(),
            "claim_ttl_ms": claim_ttl.as_millis(),
            "minimum_ttl_ms": expected_ttl.as_millis(),
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;

    fn manifest() -> ScenarioManifest {
        ScenarioManifest {
            schema_version: 1,
            seed: 4_815_162_342,
            backend: Backend::Memory,
            scenarios: vec![SCENARIO],
        }
    }

    #[tokio::test]
    async fn recurring_dispatch_evidence_replays_and_stale_index_fails_the_gate() {
        let baseline = super::super::run(manifest()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        let replay = super::super::run(manifest()).await.unwrap();
        assert!(replay.same_evidence(&baseline));

        let mut mutated = super::super::run(manifest()).await.unwrap();
        mutated.invariants.clear();
        mutated.trace.clear();
        run_with(&mut mutated, Mutation::RestoreStaleDueIndex)
            .await
            .unwrap();
        assert!(
            mutated
                .invariants
                .iter()
                .any(|check| check.name == "lease_expiry_no_redelivery" && !check.passed)
        );
        let score = evaluation::grade(SCENARIO, &mutated);
        assert!(!score.passed);
        assert!(score.gates.iter().any(|gate| !gate.passed));
    }
}
