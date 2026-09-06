//! Terminal-result recovery using real selected stores and a separately observed
//! idempotent sink. Wall-clock values and random identities are excluded from replay.
use super::{Scenario, ScenarioReport, backend_config, evaluation::derived_seed};
use crate::{SimulationConfig, SimulationError};
use acteon_core::{
    Action, WorkerTask, WorkflowStatus,
    chain::{ChainConfig, ChainStepConfig, WorkerStepConfig},
};
use acteon_executor::{DeadLetterEntry, DeadLetterError, DeadLetterSink};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_state::{
    DistributedLock, KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
const SCENARIO: Scenario = Scenario::TaskHandoffRecovery;
fn error(e: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(e.to_string())
}
fn kind(name: &str) -> KeyKind {
    KeyKind::Custom(name.into())
}
fn record(report: &mut ScenarioReport, name: &str, passed: bool, evidence: &Value) {
    let detail = evidence.to_string();
    report.check(SCENARIO, name, passed, &detail);
    report.event(SCENARIO, name, &detail, 0);
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    SkipReconciliation,
    EarlyDlqAck,
    DisableSinkDedup,
}
struct Sink {
    unavailable: AtomicBool,
    lose_ack: AtomicBool,
    attempts: AtomicUsize,
    failures: AtomicUsize,
    effects: AtomicUsize,
    seen: Mutex<HashSet<String>>,
    dedup: bool,
}
#[async_trait::async_trait]
impl DeadLetterSink for Sink {
    async fn push(
        &self,
        action: Action,
        _error: String,
        _attempts: u32,
    ) -> Result<(), DeadLetterError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(DeadLetterError("injected sink outage".into()));
        }
        if !self.dedup || self.seen.lock().unwrap().insert(action.id.to_string()) {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        if self.lose_ack.swap(false, Ordering::SeqCst) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(DeadLetterError("injected lost sink acknowledgement".into()));
        }
        Ok(())
    }
    async fn drain(&self) -> Vec<DeadLetterEntry> {
        Vec::new()
    }
    async fn len(&self) -> usize {
        self.effects.load(Ordering::SeqCst)
    }
}
struct Fixture {
    state: Arc<dyn StateStore>,
    fault: Arc<FaultStore>,
    lock: Arc<dyn DistributedLock>,
    cipher: Arc<acteon_crypto::PayloadEncryptor>,
    sink: Arc<Sink>,
}
impl Fixture {
    async fn new(report: &mut ScenarioReport, mutation: Mutation) -> Result<Self, SimulationError> {
        let config = SimulationConfig::builder()
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .build();
        let (state, lock, identity) = crate::harness::create_state_backend(&config).await?;
        let state = state.ok_or_else(|| error("handoff scenario requires shared state"))?;
        report.event(SCENARIO, "backend instantiated", identity, 0);
        let fault = Arc::new(FaultStore::new(state.clone()));
        let cipher = Arc::new(acteon_crypto::PayloadEncryptor::new(
            acteon_crypto::parse_master_key(&"46".repeat(32)).map_err(error)?,
        ));
        let sink = Arc::new(Sink {
            unavailable: AtomicBool::new(false),
            lose_ack: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            seen: Mutex::new(HashSet::new()),
            dedup: mutation != Mutation::DisableSinkDedup,
        });
        Ok(Self {
            state,
            fault,
            lock,
            cipher,
            sink,
        })
    }
    fn gateway(&self) -> Result<Gateway, SimulationError> {
        let step = |queue: &str| WorkerStepConfig {
            queue: queue.into(),
            action_type: Some("job".into()),
            timeout_seconds: None,
            max_attempts: Some(1),
        };
        let chain = ChainConfig::new("handoff")
            .with_step(ChainStepConfig::new_worker(
                "first",
                step("chain"),
                json!({}),
            ))
            .with_step(ChainStepConfig::new_worker(
                "second",
                step("next"),
                json!({}),
            ));
        let rule = acteon_rules::ir::rule::Rule::new(
            "chain",
            acteon_rules::ir::expr::Expr::Bool(true),
            acteon_rules::ir::rule::RuleAction::Chain {
                chain: "handoff".into(),
            },
        );
        GatewayBuilder::new()
            .state(self.fault.clone())
            .lock(self.lock.clone())
            .payload_encryptor(self.cipher.clone())
            .dlq_sink(self.sink.clone())
            .chain(chain)
            .rules(vec![rule])
            .build()
            .map_err(error)
    }
    async fn poll(&self, tenant: &str, queue: &str) -> Result<WorkerTask, SimulationError> {
        self.gateway()?
            .poll_worker_tasks("handoffs", tenant, queue, 1, Some(60), None)
            .await
            .map_err(error)?
            .pop()
            .ok_or_else(|| error("expected task delivery"))
    }
    async fn workflow(&self) -> Result<(String, WorkerTask), SimulationError> {
        let exec = self
            .gateway()?
            .start_workflow(
                "handoffs",
                "alice",
                "flow",
                "work",
                json!({}),
                HashMap::new(),
            )
            .await
            .map_err(error)?;
        Ok((exec.execution_id, self.poll("alice", "work").await?))
    }
    async fn complete(
        &self,
        task: &WorkerTask,
        value: Value,
    ) -> Result<WorkerTask, SimulationError> {
        self.gateway()?
            .complete_worker_task(
                &task.namespace,
                &task.tenant,
                &task.task_id,
                task.lease_token
                    .as_deref()
                    .ok_or_else(|| error("lease missing"))?,
                value,
            )
            .await
            .map_err(error)
    }
    async fn load(&self, task: &WorkerTask) -> Result<WorkerTask, SimulationError> {
        self.gateway()?
            .get_worker_task(&task.namespace, &task.tenant, &task.task_id)
            .await
            .map_err(error)?
            .ok_or_else(|| error("task missing"))
    }
    async fn workflow_done(&self, id: &str) -> Result<bool, SimulationError> {
        Ok(self
            .gateway()?
            .get_workflow_execution("handoffs", "alice", id)
            .await
            .map_err(error)?
            .is_some_and(|exec| exec.status == WorkflowStatus::Completed))
    }
    async fn repair(&self) -> Result<(), SimulationError> {
        self.gateway()?
            .reconcile_worker_handoffs()
            .await
            .map_err(error)?;
        Ok(())
    }
}
pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    Box::pin(run_with(report, Mutation::None)).await
}
#[allow(clippy::too_many_lines)]
async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let f = Fixture::new(report, mutation).await?;
    let object = derived_seed(report.manifest.seed, 0, "handoff_object") % 10_000;
    let (id, task) = f.workflow().await?;
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .map_err(error)?;
    let lost = f.complete(&task, json!({"object":object})).await.is_err();
    let retained = f
        .load(&task)
        .await?
        .handoff
        .is_some_and(|h| h.workflow_pending);
    if mutation != Mutation::SkipReconciliation {
        f.repair().await?;
    }
    let done = f.workflow_done(&id).await?;
    record(
        report,
        "terminal_ack_loss",
        lost && retained && done,
        &json!({"ack_lost":lost,"handoff_retained":retained,"workflow_completed":done,"object":object}),
    );
    if !done {
        f.repair().await?;
    }

    let (id, task) = f.workflow().await?;
    f.fault
        .fail_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .map_err(error)?;
    f.complete(&task, json!({"done":true})).await?;
    let pending = f
        .load(&task)
        .await?
        .handoff
        .is_some_and(|h| h.workflow_pending);
    f.repair().await?;
    let done = f.workflow_done(&id).await?;
    record(
        report,
        "receiver_outage",
        pending && done,
        &json!({"pending_after_outage":pending,"workflow_completed":done}),
    );

    let gateway = f.gateway()?;
    let outcome = gateway
        .dispatch(
            Action::new("handoffs", "alice", "worker", "chain", json!({})),
            None,
        )
        .await
        .map_err(error)?;
    let acteon_core::ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
        return Err(error("chain did not start"));
    };
    gateway
        .advance_chain("handoffs", "alice", &chain_id)
        .await
        .map_err(error)?;
    let task = f.poll("alice", "chain").await?;
    f.fault
        .fail_next(
            KeyKind::PendingChains,
            WriteOperation::IndexChainReady,
            FaultTiming::Before,
        )
        .map_err(error)?;
    f.complete(&task, json!({"ok":true})).await?;
    let pending = f
        .load(&task)
        .await?
        .handoff
        .is_some_and(|h| h.chain_pending);
    f.repair().await?;
    let state = gateway
        .get_chain_status("handoffs", "alice", &chain_id)
        .await
        .map_err(error)?
        .ok_or_else(|| error("chain missing"))?;
    let ready = f
        .state
        .get_ready_chains(gateway.clock().now().timestamp_millis())
        .await
        .map_err(error)?
        .iter()
        .any(|key| key.ends_with(&chain_id));
    record(
        report,
        "chain_ready_repair",
        pending && ready && state.current_step == 1 && state.execution_path.len() == 2,
        &json!({"pending_after_write":pending,"ready":ready,"next_step":state.current_step,"path_length":state.execution_path.len()}),
    );

    f.sink.unavailable.store(true, Ordering::SeqCst);
    gateway
        .enqueue_worker_task(WorkerTask::new(
            "handoffs",
            "alice",
            "dlq",
            "job",
            json!({"object":object}),
        ))
        .await
        .map_err(error)?;
    let task = f.poll("alice", "dlq").await?;
    gateway
        .fail_worker_task(
            "handoffs",
            "alice",
            &task.task_id,
            task.lease_token
                .as_deref()
                .ok_or_else(|| error("lease missing"))?,
            "failure",
            false,
        )
        .await
        .map_err(error)?;
    let mut retained = f.load(&task).await?;
    let pending = retained
        .handoff
        .as_ref()
        .is_some_and(|h| h.dlq_pending && h.completed_at.is_none());
    record(
        report,
        "pending_outage",
        pending && f.sink.effects.load(Ordering::SeqCst) == 0,
        &json!({"pending":pending,"effects":f.sink.effects.load(Ordering::SeqCst)}),
    );
    if mutation == Mutation::EarlyDlqAck {
        let handoff = retained
            .handoff
            .as_mut()
            .ok_or_else(|| error("handoff missing"))?;
        handoff.dlq_pending = false;
        handoff.completed_at = Some(gateway.clock().now());
        let raw = f
            .cipher
            .encrypt_str(&serde_json::to_string(&retained).map_err(error)?)
            .map_err(error)?;
        f.state
            .set(
                &StateKey::new("handoffs", "alice", kind("worker_task"), &task.task_id),
                &raw,
                None,
            )
            .await
            .map_err(error)?;
    }
    f.sink.unavailable.store(false, Ordering::SeqCst);
    f.sink.lose_ack.store(true, Ordering::SeqCst);
    let lost_ack = f.repair().await.is_err();
    f.repair().await?;
    let effects = f.sink.effects.load(Ordering::SeqCst);
    let attempts = f.sink.attempts.load(Ordering::SeqCst);
    let done = f
        .load(&task)
        .await?
        .handoff
        .is_some_and(|h| h.completed_at.is_some());
    record(
        report,
        "dlq_ack_loss",
        lost_ack && effects == 1 && attempts == 3 && done,
        &json!({"ack_lost":lost_ack,"effects":effects,"attempts":attempts,"delivery_complete":done}),
    );

    let (id, _owner) = f.workflow().await?;
    gateway
        .enqueue_worker_task(
            WorkerTask::new("handoffs", "bob", "foreign", "job", json!({})).for_workflow(&id),
        )
        .await
        .map_err(error)?;
    let foreign = f.poll("bob", "foreign").await?;
    f.complete(&foreign, json!({"forged":true})).await?;
    let unchanged = !f.workflow_done(&id).await?;
    let retained = f
        .load(&foreign)
        .await?
        .handoff
        .is_some_and(|h| h.workflow_pending);
    record(
        report,
        "workflow_scope",
        unchanged && retained,
        &json!({"foreign_receiver_unchanged":unchanged,"undelivered_result_retained":retained}),
    );

    let mut records = 0;
    let mut encrypted = 0;
    for key_kind in [kind("worker_task"), kind("workflow_exec"), KeyKind::Chain] {
        for (_, raw) in f.state.scan_keys_by_kind(key_kind).await.map_err(error)? {
            records += 1;
            if acteon_crypto::is_encrypted(&raw) {
                encrypted += 1;
            }
        }
    }
    record(
        report,
        "encrypted_progress",
        records > 0 && records == encrypted,
        &json!({"records":records,"encrypted":encrypted}),
    );
    record(
        report,
        "faults_consumed",
        f.fault.consumed() == 3 && f.sink.failures.load(Ordering::SeqCst) == 2,
        &json!({"store_faults":f.fault.consumed(),"sink_faults":f.sink.failures.load(Ordering::SeqCst)}),
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;
    #[tokio::test]
    async fn handoff_replay_and_safety_mutations() {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 42,
            backend: Backend::Memory,
            scenarios: vec![SCENARIO],
        };
        let baseline = super::super::run(manifest.clone()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        assert!(
            super::super::run(manifest)
                .await
                .unwrap()
                .same_evidence(&baseline)
        );
        for (mutation, check) in [
            (Mutation::SkipReconciliation, "terminal_ack_loss"),
            (Mutation::EarlyDlqAck, "dlq_ack_loss"),
            (Mutation::DisableSinkDedup, "dlq_ack_loss"),
        ] {
            let mut report = ScenarioReport {
                schema_version: baseline.schema_version,
                manifest: baseline.manifest.clone(),
                manifest_sha256: baseline.manifest_sha256.clone(),
                implementation_version: baseline.implementation_version.clone(),
                invariants: Vec::new(),
                trace: Vec::new(),
            };
            Box::pin(run_with(&mut report, mutation)).await.unwrap();
            assert!(
                report
                    .invariants
                    .iter()
                    .any(|c| c.name == check && !c.passed)
            );
            let score = evaluation::grade(SCENARIO, &report);
            assert!(!score.passed);
            assert!(score.gates.iter().any(|gate| !gate.passed));
        }
    }
}
