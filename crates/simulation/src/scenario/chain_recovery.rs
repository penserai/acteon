//! Chain-index recovery over the selected real store and lock backend.
//!
//! The primary chain row is authoritative; this scenario deliberately loses
//! pending/ready discovery after durable writes, then checks semantic replay.

use std::sync::Arc;

use acteon_core::{
    Action, ChainStatus,
    chain::{ChainConfig, ChainStepConfig, SignalStepConfig},
};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state::{
    DistributedLock, KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use serde_json::{Value, json};

use super::{Scenario, ScenarioReport, backend_config};
use crate::{SimulationConfig, SimulationError};

const SCENARIO: Scenario = Scenario::ChainDiscoveryRecovery;

fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(error.to_string())
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
    KeepOrphan,
    Plaintext,
}

struct Fixture {
    state: Arc<dyn StateStore>,
    fault: Arc<FaultStore>,
    lock: Arc<dyn DistributedLock>,
    cipher: Arc<acteon_crypto::PayloadEncryptor>,
    mutation: Mutation,
}

impl Fixture {
    async fn new(report: &mut ScenarioReport, mutation: Mutation) -> Result<Self, SimulationError> {
        let config = SimulationConfig::builder()
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .build();
        let (state, lock, identity) = crate::harness::create_state_backend(&config).await?;
        let state = state.ok_or_else(|| error("chain recovery requires shared state"))?;
        report.event(SCENARIO, "backend instantiated", identity, 0);
        let fault = Arc::new(FaultStore::new(state.clone()));
        let cipher = Arc::new(acteon_crypto::PayloadEncryptor::new(
            acteon_crypto::parse_master_key(&"48".repeat(32)).map_err(error)?,
        ));
        Ok(Self {
            state,
            fault,
            lock,
            cipher,
            mutation,
        })
    }

    fn gateway(&self) -> Result<Gateway, SimulationError> {
        let chain = ChainConfig::new("recovery").with_step(ChainStepConfig::new_wait_for_signal(
            "wait",
            SignalStepConfig {
                signal_name: "continue".into(),
                timeout_seconds: None,
                on_timeout: None,
            },
        ));
        let rule = Rule::new(
            "start",
            Expr::Bool(true),
            RuleAction::Chain {
                chain: "recovery".into(),
            },
        );
        let mut builder = GatewayBuilder::new()
            .state(self.fault.clone())
            .lock(self.lock.clone())
            .chain(chain)
            .rules(vec![rule]);
        if self.mutation != Mutation::Plaintext {
            builder = builder.payload_encryptor(self.cipher.clone());
        }
        builder.build().map_err(error)
    }

    async fn id(&self) -> Result<String, SimulationError> {
        let rows = self
            .state
            .scan_keys_by_kind(KeyKind::Chain)
            .await
            .map_err(error)?;
        let key = rows
            .first()
            .map(|(key, _)| key)
            .ok_or_else(|| error("chain source missing"))?;
        key.rsplit(':')
            .next()
            .map(str::to_owned)
            .ok_or_else(|| error("invalid chain key"))
    }

    fn pending(id: &str) -> StateKey {
        StateKey::new("chain-recovery", "alice", KeyKind::PendingChains, id)
    }

    async fn ready(&self, gateway: &Gateway, id: &str) -> Result<bool, SimulationError> {
        Ok(self
            .state
            .get_ready_chains(gateway.clock().now().timestamp_millis())
            .await
            .map_err(error)?
            .iter()
            .any(|key| key.ends_with(id)))
    }
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    Box::pin(run_with(report, Mutation::None)).await
}

#[allow(clippy::too_many_lines)]
async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let f = Fixture::new(report, mutation).await?;
    let gateway = f.gateway()?;
    f.fault
        .fail_next(
            KeyKind::PendingChains,
            WriteOperation::Set,
            FaultTiming::Before,
        )
        .map_err(error)?;
    let start_failed = gateway
        .dispatch(
            Action::new("chain-recovery", "alice", "chain", "start", json!({})),
            None,
        )
        .await
        .is_err();
    let id = f.id().await?;
    let missing_before = f
        .state
        .get(&Fixture::pending(&id))
        .await
        .map_err(error)?
        .is_none();
    if mutation != Mutation::SkipReconciliation {
        gateway.reconcile_chain_discovery().await.map_err(error)?;
    }
    let initial_visible = f
        .state
        .get(&Fixture::pending(&id))
        .await
        .map_err(error)?
        .is_some()
        && f.ready(&gateway, &id).await?;
    record(
        report,
        "initial_discovery",
        start_failed && missing_before && initial_visible,
        &json!({"start_failed":start_failed,"missing_before":missing_before,"visible_after":initial_visible}),
    );
    if !initial_visible {
        gateway.reconcile_chain_discovery().await.map_err(error)?;
    }

    gateway
        .advance_chain("chain-recovery", "alice", &id)
        .await
        .map_err(error)?;
    let waiting = gateway
        .get_chain_status("chain-recovery", "alice", &id)
        .await
        .map_err(error)?
        .is_some_and(|chain| chain.status == ChainStatus::WaitingSignal);
    gateway
        .signal_chain(
            "chain-recovery",
            "alice",
            &id,
            "continue",
            json!({"ok":true}),
        )
        .await
        .map_err(error)?;
    f.state
        .delete(&Fixture::pending(&id))
        .await
        .map_err(error)?;
    f.state
        .remove_chain_ready_index(&Fixture::pending(&id))
        .await
        .map_err(error)?;
    if mutation != Mutation::SkipReconciliation {
        gateway.reconcile_chain_discovery().await.map_err(error)?;
    }
    let signal_visible = f
        .state
        .get(&Fixture::pending(&id))
        .await
        .map_err(error)?
        .is_some()
        && f.ready(&gateway, &id).await?;
    record(
        report,
        "buffered_signal_wake",
        waiting && signal_visible,
        &json!({"waiting":waiting,"visible_after":signal_visible}),
    );
    if !signal_visible {
        gateway.reconcile_chain_discovery().await.map_err(error)?;
    }

    gateway
        .advance_chain("chain-recovery", "alice", &id)
        .await
        .map_err(error)?;
    f.state
        .set(&Fixture::pending(&id), "stale", None)
        .await
        .map_err(error)?;
    f.state
        .index_chain_ready(
            &Fixture::pending(&id),
            gateway.clock().now().timestamp_millis(),
        )
        .await
        .map_err(error)?;
    if mutation != Mutation::KeepOrphan {
        gateway.reconcile_chain_discovery().await.map_err(error)?;
    }
    let terminal = gateway
        .get_chain_status("chain-recovery", "alice", &id)
        .await
        .map_err(error)?
        .is_some_and(|chain| chain.status == ChainStatus::Completed);
    let orphan_pruned = f
        .state
        .get(&Fixture::pending(&id))
        .await
        .map_err(error)?
        .is_none()
        && !f.ready(&gateway, &id).await?;
    record(
        report,
        "terminal_orphan_pruned",
        terminal && orphan_pruned,
        &json!({"terminal":terminal,"orphan_pruned":orphan_pruned}),
    );

    let encrypted = f
        .state
        .scan_keys_by_kind(KeyKind::Chain)
        .await
        .map_err(error)?
        .iter()
        .all(|(_, raw)| acteon_crypto::is_encrypted(raw));
    record(
        report,
        "encrypted_primary_state",
        encrypted,
        &json!({"encrypted":encrypted}),
    );
    record(
        report,
        "faults_consumed",
        f.fault.consumed() == 1,
        &json!({"consumed":f.fault.consumed()}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_replays_and_unsafe_mutations_fail_gates() {
        let manifest = super::super::ScenarioManifest {
            schema_version: 1,
            seed: 20_260_907,
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
            (Mutation::SkipReconciliation, "initial_discovery"),
            (Mutation::KeepOrphan, "terminal_orphan_pruned"),
            (Mutation::Plaintext, "encrypted_primary_state"),
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
