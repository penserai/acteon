//! Restart and lost-outcome evidence using real workflow and scheduled delivery APIs.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::Duration;

use acteon_core::{Action, ActionOutcome, ProviderResponse, WorkflowDirective, WorkflowStatus};
use acteon_gateway::{
    BackgroundConfig, BackgroundJob, BackgroundProcessorBuilder, Gateway, GatewayBuilder,
};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::RuleFrontend;
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};
use serde_json::{Value, json};

use super::{
    Scenario, ScenarioReport, evaluation::derived_seed, scheduling_fault::CompletionFault,
};
use crate::SimulationError;

const SCENARIO: Scenario = Scenario::DurableScheduling;

fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(error.to_string())
}
fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("fixed epoch"),
    ))
}
fn advance(clock: &ManualClock, ms: u64) -> Result<(), SimulationError> {
    clock.advance_to(Duration::from_millis(ms)).map_err(error)
}
fn builder(clock: &Arc<ManualClock>, store: Arc<dyn StateStore>) -> GatewayBuilder {
    GatewayBuilder::new()
        .clock(clock.clone())
        .state(store)
        .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
}
fn record(report: &mut ScenarioReport, name: &str, passed: bool, evidence: &Value, calls: usize) {
    let detail = evidence.to_string();
    report.check(SCENARIO, name, passed, &detail);
    report.event(SCENARIO, name, &detail, calls);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    FrozenQueueClock,
    DisableIdempotency,
    SkipReconciliation,
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}
async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    deployment(report, mutation).await?;
    scheduling(report, mutation).await
}

// Preserve the ordered restart/checkpoint/timer observations as one scenario.
#[allow(clippy::too_many_lines)]
async fn deployment(
    report: &mut ScenarioReport,
    mutation: Mutation,
) -> Result<(), SimulationError> {
    let clock = clock();
    let epoch = clock.now();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let initial_clock = if mutation == Mutation::FrozenQueueClock {
        Arc::new(ManualClock::new(epoch))
    } else {
        clock.clone()
    };
    let gateway = builder(&initial_clock, store.clone())
        .build()
        .map_err(error)?;
    let release = derived_seed(report.manifest.seed, 0, "deployment_release") % 10_000;
    let execution = gateway
        .start_workflow(
            "durable",
            "alice",
            "deploy",
            "deployments",
            json!({"release":release}),
            HashMap::new(),
        )
        .await
        .map_err(error)?;
    let first = gateway
        .poll_worker_tasks("durable", "alice", "deployments", 1, Some(1), Some("old"))
        .await
        .map_err(error)?
        .remove(0);
    advance(&clock, 500)?;
    let checkpoint = gateway
        .record_workflow_checkpoint(
            "durable",
            "alice",
            &execution.execution_id,
            "build",
            json!({"artifact":release}),
        )
        .await
        .map_err(error)?;
    advance(&clock, 1_000)?;
    let expired_denied = gateway
        .heartbeat_worker_task(
            "durable",
            "alice",
            &first.task_id,
            first.lease_token.as_deref().expect("lease"),
            Some(1),
        )
        .await
        .is_err();
    record(
        report,
        "expired_owner_denied",
        expired_denied,
        &json!({"at_ms":1000,"denied_before_reaping":expired_denied}),
        0,
    );
    drop(gateway);
    let recovered = builder(&clock, store.clone()).build().map_err(error)?;
    let at_expiry = recovered
        .poll_worker_tasks("durable", "alice", "deployments", 1, Some(1), Some("new"))
        .await
        .map_err(error)?;
    advance(&clock, 2_999)?;
    let before_backoff = recovered
        .poll_worker_tasks("durable", "alice", "deployments", 1, Some(1), Some("new"))
        .await
        .map_err(error)?;
    advance(&clock, 3_000)?;
    let next = recovered
        .poll_worker_tasks("durable", "alice", "deployments", 1, Some(1), Some("new"))
        .await
        .map_err(error)?
        .remove(0);
    let replayed = recovered
        .record_workflow_checkpoint(
            "durable",
            "alice",
            &execution.execution_id,
            "build",
            json!({"artifact":"must not replace persisted build"}),
        )
        .await
        .map_err(error)?;
    let other_tenant = recovered
        .get_workflow_execution("durable", "bob", &execution.execution_id)
        .await
        .map_err(error)?;
    record(
        report,
        "checkpoint_recovery",
        at_expiry.is_empty()
            && before_backoff.is_empty()
            && next.attempt == 2
            && next.lease_token != first.lease_token
            && execution.created_at == epoch
            && first.created_at == epoch
            && replayed.data == checkpoint.data
            && replayed.recorded_at == checkpoint.recorded_at
            && checkpoint.recorded_at == epoch + chrono::Duration::milliseconds(500)
            && other_tenant.is_none(),
        &json!({"release":release,"attempt":next.attempt,"at_expiry":at_expiry.len(),
            "before_backoff":before_backoff.len(),"checkpoint":replayed.data,
            "checkpoint_at":replayed.recorded_at,"other_tenant_visible":other_tenant.is_some()}),
        0,
    );
    recovered
        .complete_worker_task(
            "durable",
            "alice",
            &next.task_id,
            next.lease_token.as_deref().expect("lease"),
            serde_json::to_value(WorkflowDirective::Sleep {
                checkpoint: "rollout".into(),
                seconds: 1,
            })
            .map_err(error)?,
        )
        .await
        .map_err(error)?;
    let gateway = Arc::new(tokio::sync::RwLock::new(recovered));
    let (mut worker, _shutdown) = BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(store)
        .group_manager(gateway.read().await.group_manager())
        .metrics(gateway.read().await.metrics_arc())
        .gateway(gateway.clone())
        .config(BackgroundConfig {
            enable_template_sync: false,
            ..Default::default()
        })
        .build()
        .map_err(error)?;
    advance(&clock, 3_999)?;
    worker
        .tick(BackgroundJob::ChainAdvance)
        .await
        .map_err(error)?;
    let before = gateway
        .read()
        .await
        .get_workflow_execution("durable", "alice", &execution.execution_id)
        .await
        .map_err(error)?
        .expect("workflow");
    advance(&clock, 4_000)?;
    worker
        .tick(BackgroundJob::ChainAdvance)
        .await
        .map_err(error)?;
    let gateway = gateway.read().await;
    let resumed = gateway
        .get_workflow_execution("durable", "alice", &execution.execution_id)
        .await
        .map_err(error)?
        .expect("workflow");
    let final_task = gateway
        .poll_worker_tasks("durable", "alice", "deployments", 1, Some(1), Some("new"))
        .await
        .map_err(error)?
        .remove(0);
    gateway
        .complete_worker_task(
            "durable",
            "alice",
            &final_task.task_id,
            final_task.lease_token.as_deref().expect("lease"),
            serde_json::to_value(WorkflowDirective::Complete {
                result: json!({"deployed":release}),
            })
            .map_err(error)?,
        )
        .await
        .map_err(error)?;
    let done = gateway
        .get_workflow_execution("durable", "alice", &execution.execution_id)
        .await
        .map_err(error)?
        .expect("workflow");
    let rollout_at = resumed
        .checkpoint("rollout")
        .map(|checkpoint| checkpoint.recorded_at);
    record(
        report,
        "workflow_timer",
        before.status == WorkflowStatus::WaitingTimer
            && resumed.status == WorkflowStatus::Running
            && rollout_at == Some(clock.now())
            && final_task.created_at == clock.now()
            && done.status == WorkflowStatus::Completed,
        &json!({"before":before.status,"at_deadline":resumed.status,"rollout_at":rollout_at,
            "continuation_created_at":final_task.created_at,"terminal":done.status}),
        0,
    );
    Ok(())
}

#[derive(Default)]
struct Ledger {
    attempts: usize,
    effects: BTreeMap<(String, String), usize>,
}
struct Effect {
    idempotent: bool,
    ledger: Mutex<Ledger>,
}
#[async_trait::async_trait]
impl DynProvider for Effect {
    fn name(&self) -> &'static str {
        "effect"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let mut ledger = self.ledger.lock().expect("ledger");
        ledger.attempts += 1;
        let count = ledger
            .effects
            .entry((
                action.tenant.to_string(),
                action.payload["object"].to_string(),
            ))
            .or_default();
        if !self.idempotent || *count == 0 {
            *count += 1;
        }
        Ok(ProviderResponse::success(json!({"accepted":true})))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
impl Effect {
    fn counts(&self) -> (usize, usize) {
        let ledger = self.ledger.lock().expect("ledger");
        (ledger.attempts, ledger.effects.values().sum())
    }
}

fn scheduled_gateway(
    clock: &Arc<ManualClock>,
    store: Arc<dyn StateStore>,
    effect: Arc<Effect>,
) -> Result<Gateway, SimulationError> {
    let rules = acteon_rules_yaml::YamlFrontend.parse("rules:\n  - name: later\n    condition: {field: action.action_type, eq: scheduled}\n    action: {type: schedule, delay_seconds: 1}\n").map_err(error)?;
    let policy = acteon_core::QuotaPolicy {
        id: "alice-budget".into(),
        namespace: "durable".into(),
        tenant: "alice".into(),
        provider: None,
        principal: None,
        per_principal: false,
        max_actions: 1,
        window: acteon_core::QuotaWindow::Hourly,
        overage_behavior: acteon_core::OverageBehavior::Block,
        enabled: true,
        created_at: clock.now(),
        updated_at: clock.now(),
        description: None,
        labels: HashMap::new(),
    };
    builder(clock, store)
        .provider(effect)
        .rules(rules)
        .quota_policies(vec![policy])
        .executor_config(acteon_executor::ExecutorConfig {
            max_retries: 0,
            ..Default::default()
        })
        .build()
        .map_err(error)
}
fn scheduled_worker(
    clock: &Arc<ManualClock>,
    store: Arc<dyn StateStore>,
    gateway: &Gateway,
) -> Result<
    (
        acteon_gateway::BackgroundProcessor,
        tokio::sync::mpsc::Receiver<acteon_gateway::background::ScheduledActionDueEvent>,
    ),
    SimulationError,
> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let (worker, _shutdown) = BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(store)
        .group_manager(gateway.group_manager())
        .metrics(gateway.metrics_arc())
        .config(BackgroundConfig {
            enable_scheduled_actions: true,
            ..Default::default()
        })
        .scheduled_action_channel(tx)
        .build()
        .map_err(error)?;
    Ok((worker, rx))
}

// Keep the effect/persistence outage and redelivery observations in causal order.
#[allow(clippy::too_many_lines)]
async fn scheduling(
    report: &mut ScenarioReport,
    mutation: Mutation,
) -> Result<(), SimulationError> {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let fault = Arc::new(CompletionFault::new(store.clone()));
    let effect = Arc::new(Effect {
        idempotent: mutation != Mutation::DisableIdempotency,
        ledger: Mutex::new(Ledger::default()),
    });
    let gateway = scheduled_gateway(&clock, fault.clone(), effect.clone())?;
    // Builder policies seed the cache; the server persists them separately.
    for policy in gateway.quota_policies() {
        let key = StateKey::new("_system", "_quotas", KeyKind::Quota, &policy.id);
        store
            .set(&key, &serde_json::to_string(&policy).map_err(error)?, None)
            .await
            .map_err(error)?;
    }
    store
        .set(
            &StateKey::new("_system", "_quotas", KeyKind::Quota, "idx:durable:alice"),
            &json!(["alice-budget"]).to_string(),
            None,
        )
        .await
        .map_err(error)?;
    let object = derived_seed(report.manifest.seed, 0, "tenant_scheduled_object") % 10_000;
    let action = Action::new(
        "durable",
        "alice",
        "effect",
        "scheduled",
        json!({"object":object}),
    );
    let ActionOutcome::Scheduled { action_id: id, .. } =
        gateway.dispatch(action, None).await.map_err(error)?
    else {
        return Err(error("expected scheduled action"));
    };
    let pending = StateKey::new("durable", "alice", KeyKind::PendingScheduled, &id);
    let payload = StateKey::new("durable", "alice", KeyKind::ScheduledAction, &id);
    // Model an interrupted discovery write while the authoritative payload survives.
    store.delete(&pending).await.map_err(error)?;
    store.remove_timeout_index(&pending).await.map_err(error)?;
    let (mut worker, mut rx) = scheduled_worker(&clock, fault.clone(), &gateway)?;
    advance(&clock, 1_000)?;
    worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .map_err(error)?;
    let before = rx.try_recv().is_err();
    if mutation != Mutation::SkipReconciliation {
        worker.tick(BackgroundJob::Cleanup).await.map_err(error)?;
    }
    worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .map_err(error)?;
    let receipt = rx.try_recv().ok();
    record(
        report,
        "index_reconciliation",
        before && receipt.is_some(),
        &json!({"no_early_handoff":before,"recovered_discovery":receipt.is_some()}),
        0,
    );
    // Continue independent checks after observing the deliberately missing repair.
    let receipt = if let Some(receipt) = receipt {
        receipt
    } else {
        worker.tick(BackgroundJob::Cleanup).await.map_err(error)?;
        worker
            .tick(BackgroundJob::ScheduledActions)
            .await
            .map_err(error)?;
        rx.try_recv().map_err(error)?
    };
    fault.arm();
    let lost_outcome = gateway.dispatch_scheduled_action(&receipt).await.is_err();
    let retained = store.get(&pending).await.map_err(error)?.is_some();
    let initial: Value =
        serde_json::from_str(&store.get(&payload).await.map_err(error)?.expect("record"))
            .map_err(error)?;
    let after_fault = effect.counts();
    drop(worker);
    drop(rx);
    drop(gateway);
    let recovered = scheduled_gateway(&clock, fault.clone(), effect.clone())?;
    let (mut worker, mut rx) = scheduled_worker(&clock, fault.clone(), &recovered)?;
    advance(&clock, 61_000)?;
    worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .map_err(error)?;
    let current = rx.try_recv().map_err(error)?;
    let stale_denied = recovered
        .dispatch_scheduled_action(&receipt)
        .await
        .map_err(error)?
        .is_none();
    let mut forged = current.clone();
    forged.tenant = "bob".into();
    let cross_tenant_denied = recovered
        .dispatch_scheduled_action(&forged)
        .await
        .map_err(error)?
        .is_none();
    let completed = matches!(
        recovered
            .dispatch_scheduled_action(&current)
            .await
            .map_err(error)?,
        Some(ActionOutcome::Executed(_))
    );
    let duplicate_denied = recovered
        .dispatch_scheduled_action(&current)
        .await
        .map_err(error)?
        .is_none();
    let final_record: Value =
        serde_json::from_str(&store.get(&payload).await.map_err(error)?.expect("record"))
            .map_err(error)?;
    let attempts_and_effects = effect.counts();
    record(
        report,
        "outcome_write_recovery",
        lost_outcome
            && retained
            && initial["completed_at"].is_null()
            && fault.failures.load(Ordering::SeqCst) == 1
            && after_fault == (1, 1)
            && stale_denied
            && duplicate_denied
            && completed
            && final_record["completed_at"] == json!(clock.now())
            && store.get(&pending).await.map_err(error)?.is_none(),
        &json!({"write_faults":fault.failures.load(Ordering::SeqCst),"lost_outcome":lost_outcome,
            "discovery_retained":retained,"after_fault":after_fault,"stale_denied":stale_denied,
            "duplicate_denied":duplicate_denied,"completed":completed,"completed_at":final_record["completed_at"]}),
        attempts_and_effects.0,
    );
    record(
        report,
        "one_effect_after_retry",
        attempts_and_effects == (2, 1),
        &json!({"attempts":attempts_and_effects.0,"effects":attempts_and_effects.1,"object":object}),
        attempts_and_effects.0,
    );
    let mut blocked = 0;
    for marker in [
        "_scheduled_dispatch",
        "_recurring_dispatch",
        "_group_dispatch",
    ] {
        let action = Action::new(
            "durable",
            "alice",
            "effect",
            "scheduled",
            json!({marker:true,"object":"forbidden"}),
        );
        blocked += usize::from(matches!(
            recovered.dispatch(action, None).await.map_err(error)?,
            ActionOutcome::QuotaExceeded { .. }
        ));
    }
    let bob = Action::new(
        "durable",
        "bob",
        "effect",
        "scheduled",
        json!({"object":object}),
    );
    let scheduled = matches!(
        recovered.dispatch(bob, None).await.map_err(error)?,
        ActionOutcome::Scheduled { .. }
    );
    advance(&clock, 62_000)?;
    worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .map_err(error)?;
    let bob = rx.try_recv().map_err(error)?;
    let bob_completed = matches!(
        recovered
            .dispatch_scheduled_action(&bob)
            .await
            .map_err(error)?,
        Some(ActionOutcome::Executed(_))
    );
    let ledger = effect.ledger.lock().expect("ledger");
    let bob_effects = ledger
        .effects
        .get(&("bob".into(), json!(object).to_string()))
        .copied()
        .unwrap_or(0);
    record(
        report,
        "tenant_isolation",
        cross_tenant_denied && blocked == 3 && scheduled && bob_completed && bob_effects == 1,
        &json!({"cross_tenant_denied":cross_tenant_denied,"quota_markers_blocked":blocked,
            "bob_scheduled":scheduled,"bob_completed":bob_completed,"bob_effects":bob_effects}),
        ledger.attempts,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;
    #[tokio::test]
    async fn durable_evidence_replays_and_fault_mutations_fail_safety_gates() {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 42,
            backend: Backend::Memory,
            scenarios: vec![SCENARIO],
        };
        let baseline = super::super::run(manifest.clone()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        let replay = super::super::run(manifest).await.unwrap();
        assert!(replay.same_evidence(&baseline));
        for (mutation, failed) in [
            (Mutation::FrozenQueueClock, "expired_owner_denied"),
            (Mutation::DisableIdempotency, "one_effect_after_retry"),
            (Mutation::SkipReconciliation, "index_reconciliation"),
        ] {
            let mut report = super::super::run(baseline.manifest.clone()).await.unwrap();
            report.invariants.clear();
            report.trace.clear();
            run_with(&mut report, mutation).await.unwrap();
            assert!(
                report
                    .invariants
                    .iter()
                    .any(|check| check.name == failed && !check.passed),
                "{:?}",
                report.invariants
            );
            let score = evaluation::grade(SCENARIO, &report);
            assert!(!score.passed);
            assert!(score.gates.iter().any(|gate| !gate.passed));
        }
    }
}
