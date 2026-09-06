//! Queue recovery over the selected real state/lock backend, with controlled
//! write interruptions. Replay compares semantic observations, not wall time.

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{WorkerTask, WorkerTaskStatus};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_state::{
    DistributedLock, KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use serde_json::{Value, json};

use super::{Scenario, ScenarioReport, backend_config, evaluation::derived_seed};
use crate::{SimulationConfig, SimulationError};

const SCENARIO: Scenario = Scenario::QueueRecovery;
fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(error.to_string())
}
fn kind(name: &str) -> KeyKind {
    KeyKind::Custom(name.into())
}
fn index(tenant: &str, queue: &str, id: &str) -> StateKey {
    StateKey::new(
        "queues",
        tenant,
        kind("queue_pending"),
        format!("{queue}:{id}"),
    )
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
    DropRetryDiscovery,
    Plaintext,
}

struct Fixture {
    store: Arc<dyn StateStore>,
    fault: Arc<FaultStore>,
    lock: Arc<dyn DistributedLock>,
    encryptor: Arc<acteon_crypto::PayloadEncryptor>,
    mutation: Mutation,
}
impl Fixture {
    async fn new(report: &mut ScenarioReport, mutation: Mutation) -> Result<Self, SimulationError> {
        let config = SimulationConfig::builder()
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .build();
        let (store, lock, identity) = crate::harness::create_state_backend(&config).await?;
        let store = store.ok_or_else(|| error("queue scenario requires shared state"))?;
        report.event(SCENARIO, "backend instantiated", identity, 0);
        let fault = Arc::new(FaultStore::new(store.clone()));
        let encryptor = Arc::new(acteon_crypto::PayloadEncryptor::new(
            acteon_crypto::parse_master_key(&"44".repeat(32)).map_err(error)?,
        ));
        Ok(Self {
            store,
            fault,
            lock,
            encryptor,
            mutation,
        })
    }
    fn gateway(&self) -> Result<Gateway, SimulationError> {
        let mut builder = GatewayBuilder::new()
            .state(self.fault.clone())
            .lock(self.lock.clone());
        if self.mutation != Mutation::Plaintext {
            builder = builder.payload_encryptor(self.encryptor.clone());
        }
        builder.build().map_err(error)
    }
    async fn poll(&self, tenant: &str, queue: &str) -> Result<Vec<WorkerTask>, SimulationError> {
        self.gateway()?
            .poll_worker_tasks("queues", tenant, queue, 1, Some(60), Some("worker"))
            .await
            .map_err(error)
    }
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    Box::pin(run_with(report, Mutation::None)).await
}

// Keep the explicitly ordered persistence boundaries and observations together.
#[allow(clippy::too_many_lines)]
async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let f = Fixture::new(report, mutation).await?;
    let gateway = f.gateway()?;
    let object = derived_seed(report.manifest.seed, 0, "queue_object") % 10_000;
    let task = WorkerTask::new(
        "queues",
        "alice",
        "work",
        "deploy",
        json!({"object":object}),
    );
    let pending = index("alice", "work", &task.task_id);
    f.fault
        .fail_next(
            kind("queue_pending"),
            WriteOperation::Set,
            FaultTiming::Before,
        )
        .map_err(error)?;
    let enqueue_failed = gateway.enqueue_worker_task(task.clone()).await.is_err();
    let before = f.poll("alice", "work").await?;
    // Probe scope while Alice's authoritative row is still pending, so a live
    // lease cannot mask a missing queue-identity check.
    scope(report, &f, &task).await?;
    drop(gateway);
    let recovered = f.gateway()?;
    if mutation != Mutation::SkipReconciliation {
        recovered
            .reconcile_worker_task_indexes()
            .await
            .map_err(error)?;
    }
    let mut deliveries = f.poll("alice", "work").await?;
    record(
        report,
        "enqueue_gap",
        enqueue_failed && before.is_empty() && deliveries.len() == 1,
        &json!({"enqueue_error":enqueue_failed,"before_repair":before.len(),"after_repair":deliveries.len()}),
    );
    // Continue independent checks after observing the deliberately skipped repair.
    if deliveries.is_empty() {
        recovered
            .reconcile_worker_task_indexes()
            .await
            .map_err(error)?;
        deliveries = f.poll("alice", "work").await?;
    }
    let first = deliveries
        .pop()
        .ok_or_else(|| error("missing repaired task"))?;
    let duplicate_denied = recovered.enqueue_worker_task(task.clone()).await.is_err();
    let persisted = recovered
        .get_worker_task("queues", "alice", &task.task_id)
        .await
        .map_err(error)?
        .ok_or_else(|| error("missing task"))?;
    record(
        report,
        "duplicate_id_denied",
        duplicate_denied && persisted.lease_token == first.lease_token && persisted.attempt == 1,
        &json!({"denied":duplicate_denied,"owner_preserved":persisted.lease_token==first.lease_token,"attempt":persisted.attempt}),
    );
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .map_err(error)?;
    let retry_error = recovered
        .fail_worker_task(
            "queues",
            "alice",
            &task.task_id,
            first
                .lease_token
                .as_deref()
                .ok_or_else(|| error("missing lease"))?,
            "transient",
            true,
        )
        .await
        .is_err();
    let retry = recovered
        .get_worker_task("queues", "alice", &task.task_id)
        .await
        .map_err(error)?
        .ok_or_else(|| error("missing retry record"))?;
    if mutation == Mutation::DropRetryDiscovery {
        f.store.delete(&pending).await.map_err(error)?;
    }
    let retained = f.store.get(&pending).await.map_err(error)?.is_some();
    // This all-backend suite uses real clocks. Exact expiry/backoff boundaries
    // are exercised separately by ManualClock gateway contracts.
    recovered.clock().sleep(Duration::from_millis(2100)).await;
    let mut deliveries = f.poll("alice", "work").await?;
    let attempt = deliveries.first().map(|task| task.attempt);
    record(
        report,
        "retry_ack_loss",
        retry_error && retry.status == WorkerTaskStatus::Pending && retained && attempt == Some(2),
        &json!({"lost_ack":retry_error,"persisted_status":retry.status,"discovery_retained":retained,"next_attempt":attempt}),
    );
    if deliveries.is_empty() {
        recovered
            .reconcile_worker_task_indexes()
            .await
            .map_err(error)?;
        deliveries = f.poll("alice", "work").await?;
    }
    let next = deliveries
        .pop()
        .ok_or_else(|| error("missing retry delivery"))?;
    f.store.delete(&pending).await.map_err(error)?;
    let legacy = StateKey::new(
        "queues",
        "alice",
        kind("queue_leased"),
        format!("work:{}", task.task_id),
    );
    f.store
        .set(&legacy, "not authoritative", None)
        .await
        .map_err(error)?;
    recovered
        .reconcile_worker_task_indexes()
        .await
        .map_err(error)?;
    let repaired = f.store.get(&pending).await.map_err(error)?.is_some();
    let current = recovered
        .get_worker_task("queues", "alice", &task.task_id)
        .await
        .map_err(error)?
        .ok_or_else(|| error("missing lease"))?;
    record(
        report,
        "legacy_index_repair",
        repaired && current.lease_token == next.lease_token,
        &json!({"discovery_repaired":repaired,"owner_preserved":current.lease_token==next.lease_token}),
    );

    f.fault
        .fail_next(
            kind("queue_pending"),
            WriteOperation::Delete,
            FaultTiming::Before,
        )
        .map_err(error)?;
    let done = recovered
        .complete_worker_task(
            "queues",
            "alice",
            &task.task_id,
            next.lease_token
                .as_deref()
                .ok_or_else(|| error("missing lease"))?,
            json!({"object":object,"done":true}),
        )
        .await
        .map_err(error)?;
    let stale = f.store.get(&pending).await.map_err(error)?.is_some();
    recovered
        .reconcile_worker_task_indexes()
        .await
        .map_err(error)?;
    let removed = f.store.get(&pending).await.map_err(error)?.is_none()
        && f.store.get(&legacy).await.map_err(error)?.is_none();
    let no_redelivery = f.poll("alice", "work").await?.is_empty();
    record(
        report,
        "terminal_cleanup",
        done.status == WorkerTaskStatus::Completed
            && stale
            && removed
            && no_redelivery
            && done.result == Some(json!({"object":object,"done":true})),
        &json!({"terminal":done.status,"cleanup_interrupted":stale,"indexes_removed":removed,"no_redelivery":no_redelivery}),
    );
    let rows = f
        .store
        .scan_keys_by_kind(kind("worker_task"))
        .await
        .map_err(error)?;
    let encrypted = rows
        .iter()
        .filter(|(_, raw)| acteon_crypto::is_encrypted(raw))
        .count();
    record(
        report,
        "payload_encrypted",
        rows.len() == 2 && encrypted == 2,
        &json!({"records":rows.len(),"encrypted":encrypted}),
    );
    record(
        report,
        "faults_consumed",
        f.fault.consumed() == 3,
        &json!({"consumed":f.fault.consumed(),"expected":3}),
    );
    Ok(())
}

async fn scope(
    report: &mut ScenarioReport,
    f: &Fixture,
    task: &WorkerTask,
) -> Result<(), SimulationError> {
    f.store
        .set(&index("alice", "public", &task.task_id), "active", None)
        .await
        .map_err(error)?;
    let wrong_queue = f.poll("alice", "public").await?.len();
    f.store
        .set(&index("bob", "work", &task.task_id), "active", None)
        .await
        .map_err(error)?;
    let foreign = f.poll("bob", "work").await?.len();
    let mut bob = task.clone();
    bob.tenant = "bob".into();
    let gateway = f.gateway()?;
    gateway.enqueue_worker_task(bob).await.map_err(error)?;
    let tasks = f.poll("bob", "work").await?;
    let isolated = tasks.len() == 1 && tasks[0].tenant == "bob";
    if let Some(bob) = tasks.first() {
        gateway
            .complete_worker_task(
                "queues",
                "bob",
                &bob.task_id,
                bob.lease_token
                    .as_deref()
                    .ok_or_else(|| error("missing bob lease"))?,
                json!({"done":true}),
            )
            .await
            .map_err(error)?;
    }
    record(
        report,
        "scope_isolation",
        wrong_queue == 0 && foreign == 0 && isolated,
        &json!({"wrong_queue_deliveries":wrong_queue,"foreign_deliveries":foreign,"bob_independent":isolated}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;
    #[tokio::test]
    async fn queue_evidence_replays_and_mutations_fail_safety_gates() {
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
            (Mutation::SkipReconciliation, "enqueue_gap"),
            (Mutation::DropRetryDiscovery, "retry_ack_loss"),
            (Mutation::Plaintext, "payload_encrypted"),
        ] {
            let mut report = super::super::run(baseline.manifest.clone()).await.unwrap();
            report.invariants.clear();
            report.trace.clear();
            Box::pin(run_with(&mut report, mutation)).await.unwrap();
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
