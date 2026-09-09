//! Selected-backend recovery evidence for the terminal chain-to-task bridge.

use std::sync::Arc;

use acteon_core::{
    Action, ActionOutcome, ChainStatus, Task, TaskState,
    chain::{ChainConfig, ChainStepConfig, TimerStepConfig},
};
use acteon_gateway::{
    Gateway, GatewayBuilder,
    task_chain_bridge::link_task_to_chain,
    task_engine::{TaskEngine, TaskScope},
};
use acteon_rules::ir::{
    expr::{BinaryOp, Expr},
    rule::{Rule, RuleAction},
};
use acteon_state::{
    DistributedLock, KeyKind, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use serde_json::{Value, json};

use super::{Scenario, ScenarioReport, backend_config};
use crate::{SimulationConfig, SimulationError};

const SCENARIO: Scenario = Scenario::ChainTaskProjectionRecovery;
const NAMESPACE: &str = "chain-task-recovery";
const TENANT: &str = "alice";
const TASK_ID: &str = "linked-task";

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
    SkipRecovery,
}

struct Fixture {
    state: Arc<dyn StateStore>,
    fault: Arc<FaultStore>,
    lock: Arc<dyn DistributedLock>,
}

impl Fixture {
    async fn new(report: &mut ScenarioReport) -> Result<Self, SimulationError> {
        let config = SimulationConfig::builder()
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .build();
        let (state, lock, identity) = crate::harness::create_state_backend(&config).await?;
        let state = state.ok_or_else(|| error("task projection recovery requires shared state"))?;
        report.event(SCENARIO, "backend instantiated", identity, 0);
        let fault = Arc::new(FaultStore::new(state));
        Ok(Self {
            state: fault.clone(),
            fault,
            lock,
        })
    }

    fn gateway(&self) -> Result<Gateway, SimulationError> {
        let chain = ChainConfig::new("flow").with_step(ChainStepConfig::new_timer(
            "wait",
            TimerStepConfig {
                duration_seconds: Some(60),
                until: None,
            },
        ));
        GatewayBuilder::new()
            .state(self.state.clone())
            .lock(self.lock.clone())
            .chain(chain)
            .rules(vec![Rule::new(
                "start",
                Expr::Binary(
                    BinaryOp::Eq,
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("action".into())),
                        "action_type".into(),
                    )),
                    Box::new(Expr::String("start".into())),
                ),
                RuleAction::Chain {
                    chain: "flow".into(),
                },
            )])
            .build()
            .map_err(error)
    }
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}

async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let f = Fixture::new(report).await?;
    let engine = TaskEngine::new(f.state.clone());
    let scope = TaskScope::new(NAMESPACE, TENANT);
    engine
        .create_task(Task::new(TASK_ID, NAMESPACE, TENANT))
        .await
        .map_err(error)?;
    engine
        .transition_task(&scope, TASK_ID, TaskState::Working, None)
        .await
        .map_err(error)?;
    let gateway = f.gateway()?;
    let outcome = gateway
        .dispatch(
            Action::new(NAMESPACE, TENANT, "source", "start", json!({})),
            None,
        )
        .await
        .map_err(error)?;
    let ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
        return Err(error("expected chain start"));
    };
    link_task_to_chain(&f.state, &engine, &scope, TASK_ID, &chain_id)
        .await
        .map_err(error)?;

    // The terminal chain CAS succeeds. The following best-effort task
    // projection loses its task-row CAS, exactly the interruption the retained
    // terminal chain row is designed to recover.
    f.fault
        .fail_after_matches(
            KeyKind::A2aTask,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
            1,
        )
        .map_err(error)?;
    let cancelled = gateway
        .cancel_chain(
            NAMESPACE,
            TENANT,
            &chain_id,
            Some("operator request".into()),
            None,
        )
        .await
        .map_err(error)?;
    let before = engine
        .get_task(&scope, TASK_ID)
        .await
        .map_err(error)?
        .ok_or_else(|| error("linked task missing"))?;
    drop(gateway);
    let restarted = f.gateway()?;
    let recovered = if mutation == Mutation::SkipRecovery {
        0
    } else {
        restarted
            .reconcile_chain_task_projections()
            .await
            .map_err(error)?
    };
    let after = engine
        .get_task(&scope, TASK_ID)
        .await
        .map_err(error)?
        .ok_or_else(|| error("linked task missing after recovery"))?;
    record(
        report,
        "projection_recovered",
        cancelled.status == ChainStatus::Cancelled
            && before.status.state == TaskState::Working
            && recovered == 1
            && after.status.state == TaskState::Canceled,
        &json!({
            "before": before.status.state,
            "recovered": recovered,
            "after": after.status.state,
        }),
    );
    let repeated = restarted
        .reconcile_chain_task_projections()
        .await
        .map_err(error)?;
    let settled = engine
        .get_task(&scope, TASK_ID)
        .await
        .map_err(error)?
        .ok_or_else(|| error("linked task missing after repeated recovery"))?;
    record(
        report,
        "projection_idempotent",
        settled.status.state == TaskState::Canceled && settled.history.len() == 1 && repeated == 0,
        &json!({"repeated": repeated, "history": settled.history.len()}),
    );
    record(
        report,
        "fault_consumed",
        f.fault.consumed() == 1,
        &json!({"consumed": f.fault.consumed()}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;

    #[tokio::test]
    async fn projection_recovery_replays_and_missing_sweep_fails_the_gate() {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 20_260_909,
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
        run_with(&mut mutated, Mutation::SkipRecovery)
            .await
            .unwrap();
        assert!(
            mutated
                .invariants
                .iter()
                .any(|check| check.name == "projection_recovered" && !check.passed)
        );
        assert!(!evaluation::grade(SCENARIO, &mutated).passed);
    }
}
