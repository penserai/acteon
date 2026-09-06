//! Real store/lock contention with a shortened execution lease and controlled writes.
use super::{Scenario, ScenarioReport, backend_config};
use crate::{SimulationConfig, SimulationError};
use acteon_core::{
    Action, ActionOutcome, ChainStatus,
    chain::{ChainConfig, ChainStepConfig, TimerStepConfig},
};
use acteon_gateway::{BackgroundJob, BackgroundProcessorBuilder, Gateway, GatewayBuilder};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state::{
    DistributedLock, KeyKind, LockGuard, StateError, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const SCENARIO: Scenario = Scenario::ChainWriteFencing;
fn error(value: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(value.to_string())
}
fn record(report: &mut ScenarioReport, name: &str, passed: bool, detail: &str) {
    report.check(SCENARIO, name, passed, detail);
    report.event(SCENARIO, name, detail, 0);
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    StaleWrite,
    StaleDelete,
    Plaintext,
}

/// Keep the selected backend's lock implementation; shorten only the explicitly
/// armed acquisition so the expired-owner contract need not wait thirty seconds.
struct ShortLease {
    inner: Arc<dyn DistributedLock>,
    armed: AtomicBool,
}
impl ShortLease {
    fn ttl(&self, requested: Duration) -> Duration {
        if self.armed.swap(false, Ordering::SeqCst) {
            Duration::from_millis(50)
        } else {
            requested
        }
    }
}
#[async_trait::async_trait]
impl DistributedLock for ShortLease {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        self.inner.try_acquire(name, self.ttl(ttl)).await
    }
    async fn acquire(
        &self,
        name: &str,
        ttl: Duration,
        timeout: Duration,
    ) -> Result<Box<dyn LockGuard>, StateError> {
        self.inner.acquire(name, self.ttl(ttl), timeout).await
    }
}
struct Fixture {
    state: Arc<dyn StateStore>,
    fault: Arc<FaultStore>,
    lock: Arc<ShortLease>,
    gateway: Arc<Gateway>,
    cipher: Arc<acteon_crypto::PayloadEncryptor>,
}
impl Fixture {
    async fn new(report: &mut ScenarioReport, mutation: Mutation) -> Result<Self, SimulationError> {
        let config = SimulationConfig::builder()
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .build();
        let (state, lock, identity) = crate::harness::create_state_backend(&config).await?;
        let state = state.ok_or_else(|| error("fencing scenario requires shared state"))?;
        let lock = Arc::new(ShortLease {
            inner: lock,
            armed: AtomicBool::new(false),
        });
        let fault = Arc::new(FaultStore::new(state.clone()));
        report.event(SCENARIO, "backend instantiated", identity, 0);
        let cipher = Arc::new(acteon_crypto::PayloadEncryptor::new(
            acteon_crypto::parse_master_key(&"48".repeat(32)).map_err(error)?,
        ));
        let chain = ChainConfig::new("flow").with_step(ChainStepConfig::new_timer(
            "nap",
            TimerStepConfig {
                duration_seconds: Some(60),
                until: None,
            },
        ));
        let mut builder = GatewayBuilder::new()
            .state(fault.clone())
            .lock(lock.clone())
            .chain(chain)
            .rules(vec![Rule::new(
                "start",
                Expr::Bool(true),
                RuleAction::Chain {
                    chain: "flow".into(),
                },
            )]);
        if mutation != Mutation::Plaintext {
            builder = builder.payload_encryptor(cipher.clone());
        }
        Ok(Self {
            state,
            fault,
            lock,
            gateway: Arc::new(builder.build().map_err(error)?),
            cipher,
        })
    }
    async fn start(&self) -> Result<String, SimulationError> {
        let result = self
            .gateway
            .dispatch(
                Action::new("fencing", "alice", "effect", "start", json!({})),
                None,
            )
            .await
            .map_err(error)?;
        let ActionOutcome::ChainStarted { chain_id, .. } = result else {
            return Err(error("chain did not start"));
        };
        Ok(chain_id)
    }
    fn key(id: &str) -> StateKey {
        StateKey::new("fencing", "alice", KeyKind::Chain, id)
    }
    async fn reached(&self, count: usize) -> Result<(), SimulationError> {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.fault.consumed() < count {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .map_err(error)
    }
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    Box::pin(run_with(report, Mutation::None)).await
}
#[allow(clippy::too_many_lines)]
async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let f = Fixture::new(report, mutation).await?;
    let object = super::evaluation::derived_seed(report.manifest.seed, 0, "chain-fencing-object")
        % 1_000_000;
    for (index, name) in ["state_fence", "timer_fence"].into_iter().enumerate() {
        let id = f.start().await?;
        f.gateway
            .upsert_search_attributes(
                "fencing",
                "alice",
                &id,
                HashMap::from([("object".into(), json!(object))]),
            )
            .await
            .map_err(error)?;
        let old = f
            .state
            .get(&Fixture::key(&id))
            .await
            .map_err(error)?
            .ok_or_else(|| error("missing source"))?;
        f.lock.armed.store(true, Ordering::SeqCst);
        let resume = f
            .fault
            .pause_next(
                KeyKind::Chain,
                WriteOperation::CompareAndSwap,
                FaultTiming::Before,
            )
            .map_err(error)?;
        let gateway = f.gateway.clone();
        let source_id = id.clone();
        let writer = tokio::spawn(async move {
            if index == 0 {
                gateway
                    .upsert_search_attributes(
                        "fencing",
                        "alice",
                        &source_id,
                        HashMap::from([("stale".into(), json!(true))]),
                    )
                    .await
                    .map(|_| ())
            } else {
                gateway.advance_chain("fencing", "alice", &source_id).await
            }
        });
        f.reached(index + 1).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.gateway
            .cancel_chain("fencing", "alice", &id, Some("successor".into()), None)
            .await
            .map_err(error)?;
        resume.send(()).map_err(|()| error("writer pause lost"))?;
        let rejected = writer.await.map_err(error)?.is_err();
        if mutation == Mutation::StaleWrite && index == 0 {
            f.state
                .set(&Fixture::key(&id), &old, None)
                .await
                .map_err(error)?;
        }
        let current = f
            .gateway
            .get_chain_status("fencing", "alice", &id)
            .await
            .map_err(error)?
            .ok_or_else(|| error("missing receiver"))?;
        record(
            report,
            name,
            rejected
                && current.status == ChainStatus::Cancelled
                && current.wait_state.is_none()
                && !current.search_attributes.contains_key("stale")
                && current.search_attributes.get("object") == Some(&json!(object)),
            &format!(
                "writer_rejected={rejected}; status={:?}; stale_attribute={}; object={object}",
                current.status,
                current.search_attributes.contains_key("stale")
            ),
        );
    }

    let id = f.start().await?;
    let resume = f
        .fault
        .pause_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .map_err(error)?;
    let gateway = f.gateway.clone();
    let source_id = id.clone();
    let writer = tokio::spawn(async move {
        gateway
            .upsert_search_attributes("fencing", "alice", &source_id, HashMap::new())
            .await
    });
    f.reached(3).await?;
    f.state.delete(&Fixture::key(&id)).await.map_err(error)?;
    resume.send(()).map_err(|()| error("writer pause lost"))?;
    let rejected = writer.await.map_err(error)?.is_err();
    record(
        report,
        "no_resurrection",
        rejected
            && f.state
                .get(&Fixture::key(&id))
                .await
                .map_err(error)?
                .is_none(),
        "deleted receiver remains absent after stale update",
    );

    let id = f.start().await?;
    let raw = f
        .state
        .get(&Fixture::key(&id))
        .await
        .map_err(error)?
        .ok_or_else(|| error("missing source"))?;
    let wrong = StateKey::new("fencing", "bob", KeyKind::Chain, &id);
    f.state.set(&wrong, &raw, None).await.map_err(error)?;
    let rejected = f
        .gateway
        .upsert_search_attributes("fencing", "bob", &id, HashMap::new())
        .await
        .is_err();
    record(
        report,
        "scope_guard",
        rejected && f.state.get(&wrong).await.map_err(error)? == Some(raw),
        "mismatched receiver rejected without mutation",
    );

    // Reaping an old terminal snapshot cannot delete a reset of the same chain.
    let id = f.start().await?;
    f.gateway
        .cancel_chain("fencing", "alice", &id, None, None)
        .await
        .map_err(error)?;
    let policy: acteon_core::RetentionPolicy = serde_json::from_value(json!({"id":"fencing", "namespace":"fencing", "tenant":"alice", "state_ttl_seconds":0, "created_at":chrono::Utc::now(), "updated_at":chrono::Utc::now()})).map_err(error)?;
    f.state
        .set(
            &StateKey::new("fencing", "alice", KeyKind::Retention, "fencing"),
            &serde_json::to_string(&policy).map_err(error)?,
            None,
        )
        .await
        .map_err(error)?;
    let (worker, _) = BackgroundProcessorBuilder::new()
        .config(acteon_gateway::BackgroundConfig {
            enable_retention_reaper: true,
            ..Default::default()
        })
        .state(f.fault.clone())
        .group_manager(f.gateway.group_manager())
        .metrics(f.gateway.metrics_arc())
        .build()
        .map_err(error)?;
    let mut worker = worker
        .with_payload_encryptor(f.cipher.clone())
        .with_retention_policies(HashMap::from([("fencing:alice".into(), policy)]));
    // Leave only this terminal chain eligible, making the paused deletion explicit.
    for (key, _) in f
        .state
        .scan_keys_by_kind(KeyKind::Chain)
        .await
        .map_err(error)?
    {
        let parts: Vec<_> = key.splitn(4, ':').collect();
        if parts.len() == 4 && parts[1] == "alice" && parts[3] != id {
            f.state
                .delete(&StateKey::new(parts[0], parts[1], KeyKind::Chain, parts[3]))
                .await
                .map_err(error)?;
        }
    }
    let resume = f
        .fault
        .pause_next(
            KeyKind::Chain,
            WriteOperation::CompareAndDelete,
            FaultTiming::Before,
        )
        .map_err(error)?;
    let reaper = tokio::spawn(async move { worker.tick(BackgroundJob::Retention).await });
    f.reached(4).await?;
    f.gateway
        .reset_execution("fencing", "alice", &id, "nap", None)
        .await
        .map_err(error)?;
    resume.send(()).map_err(|()| error("reaper pause lost"))?;
    reaper.await.map_err(error)?.map_err(error)?;
    if mutation == Mutation::StaleDelete {
        f.state.delete(&Fixture::key(&id)).await.map_err(error)?;
    }
    let current = f
        .gateway
        .get_chain_status("fencing", "alice", &id)
        .await
        .map_err(error)?;
    record(
        report,
        "retention_fence",
        current
            .as_ref()
            .is_some_and(|chain| chain.status == ChainStatus::Running),
        &format!(
            "status_after_reaper={:?}",
            current.as_ref().map(|chain| &chain.status)
        ),
    );

    let rows = f
        .state
        .scan_keys_by_kind(KeyKind::Chain)
        .await
        .map_err(error)?;
    record(
        report,
        "encrypted_state",
        !rows.is_empty() && rows.iter().all(|(_, raw)| acteon_crypto::is_encrypted(raw)),
        "all remaining chain records use payload encryption",
    );
    record(
        report,
        "faults_consumed",
        f.fault.consumed() == 4,
        "three paused updates and one paused conditional deletion consumed",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn fencing_replays_and_unsafe_mutations_fail_gates() {
        let manifest = super::super::ScenarioManifest {
            schema_version: 1,
            seed: 20_260_906,
            backend: super::super::Backend::Memory,
            scenarios: vec![SCENARIO],
        };
        let baseline = super::super::run(manifest.clone()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        assert_eq!(
            serde_json::to_value(&baseline).unwrap(),
            serde_json::to_value(super::super::run(manifest).await.unwrap()).unwrap()
        );
        for (mutation, gate) in [
            (Mutation::StaleWrite, "state_fence"),
            (Mutation::StaleDelete, "retention_fence"),
            (Mutation::Plaintext, "encrypted_state"),
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
                    .any(|check| check.name == gate && !check.passed)
            );
            assert!(!super::super::evaluation::grade(SCENARIO, &report).passed);
        }
    }
}
