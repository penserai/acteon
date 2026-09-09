//! Selected-backend recovery evidence for terminal cancellation notifications.

use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use acteon_core::{
    Action, ActionOutcome, ChainStatus, ProviderResponse,
    chain::{ChainConfig, ChainNotificationTarget, ChainStepConfig, TimerStepConfig},
};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::{
    expr::{BinaryOp, Expr},
    rule::{Rule, RuleAction},
};
use acteon_state::{DistributedLock, StateStore};
use serde_json::{Value, json};

use super::{Scenario, ScenarioReport, backend_config};
use crate::{SimulationConfig, SimulationError};

const SCENARIO: Scenario = Scenario::CancellationHandoffRecovery;
const NAMESPACE: &str = "cancellation-recovery";
const TENANT: &str = "alice";

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
    DisableDownstreamDedup,
}

/// A controlled notification transport. It can fail before delivery or after
/// the downstream effect but before the gateway receives a response.
/// The effect ledger models a receiver that deduplicates on the durable action
/// ID, which is the contract offered by cancellation handoffs.
struct NotificationProvider {
    unavailable: AtomicBool,
    lose_response: AtomicBool,
    attempts: AtomicUsize,
    effects: AtomicUsize,
    failures: AtomicUsize,
    delivery_ids: Mutex<Vec<String>>,
    seen: Mutex<HashSet<String>>,
    deduplicates: bool,
}

impl NotificationProvider {
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    fn effects(&self) -> usize {
        self.effects.load(Ordering::SeqCst)
    }

    fn failures(&self) -> usize {
        self.failures.load(Ordering::SeqCst)
    }

    fn delivery_ids_after(&self, start: usize) -> Vec<String> {
        self.delivery_ids.lock().expect("delivery IDs")[start..].to_vec()
    }
}

#[async_trait::async_trait]
impl DynProvider for NotificationProvider {
    fn name(&self) -> &'static str {
        "notify"
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let delivery_id = action.id.to_string();
        self.delivery_ids
            .lock()
            .expect("delivery IDs")
            .push(delivery_id.clone());
        if self.unavailable.load(Ordering::SeqCst) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(ProviderError::Connection(
                "injected notification transport outage".into(),
            ));
        }
        if !self.deduplicates || self.seen.lock().expect("effect ledger").insert(delivery_id) {
            self.effects.fetch_add(1, Ordering::SeqCst);
        }
        if self.lose_response.swap(false, Ordering::SeqCst) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(ProviderError::Connection(
                "injected transport loss after downstream effect".into(),
            ));
        }
        Ok(ProviderResponse::success(json!({"notified": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct Fixture {
    state: Arc<dyn StateStore>,
    lock: Arc<dyn DistributedLock>,
    notifications: Arc<NotificationProvider>,
}

impl Fixture {
    async fn new(report: &mut ScenarioReport, mutation: Mutation) -> Result<Self, SimulationError> {
        let config = SimulationConfig::builder()
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .build();
        let (state, lock, identity) = crate::harness::create_state_backend(&config).await?;
        let state = state.ok_or_else(|| error("cancellation recovery requires shared state"))?;
        report.event(SCENARIO, "backend instantiated", identity, 0);
        let notifications = Arc::new(NotificationProvider {
            unavailable: AtomicBool::new(false),
            lose_response: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            delivery_ids: Mutex::new(Vec::new()),
            seen: Mutex::new(HashSet::new()),
            deduplicates: mutation != Mutation::DisableDownstreamDedup,
        });
        Ok(Self {
            state,
            lock,
            notifications,
        })
    }

    fn gateway(&self) -> Result<Gateway, SimulationError> {
        let chain = ChainConfig::new("flow")
            .with_step(ChainStepConfig::new_timer(
                "wait",
                TimerStepConfig {
                    duration_seconds: Some(60),
                    until: None,
                },
            ))
            .with_on_cancel(ChainNotificationTarget {
                provider: "notify".into(),
                action_type: "chain_cancelled".into(),
            });
        GatewayBuilder::new()
            .state(self.state.clone())
            .lock(self.lock.clone())
            .provider(self.notifications.clone())
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
            .executor_config(ExecutorConfig {
                max_retries: 0,
                ..Default::default()
            })
            .build()
            .map_err(error)
    }

    async fn start(&self, gateway: &Gateway) -> Result<String, SimulationError> {
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
        Ok(chain_id)
    }
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}

#[allow(clippy::too_many_lines)]
async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let f = Fixture::new(report, mutation).await?;

    // The terminal state/outbox write succeeds, but the provider is partitioned
    // from the gateway. Reconstruct the gateway over the selected state backend
    // before reconciliation to model a process replacement rather than retained
    // in-memory control flow.
    f.notifications.unavailable.store(true, Ordering::SeqCst);
    let outage_start = f.notifications.attempts();
    let initial = f.gateway()?;
    let chain_id = f.start(&initial).await?;
    let cancelled = initial
        .cancel_chain(
            NAMESPACE,
            TENANT,
            &chain_id,
            Some("operator request".into()),
            Some("operator".into()),
        )
        .await
        .map_err(error)?;
    let outage_id = cancelled
        .cancellation_handoff
        .as_ref()
        .ok_or_else(|| error("terminal cancellation must retain a handoff"))?
        .delivery_id
        .clone();
    let pending_after_outage = cancelled
        .cancellation_handoff
        .as_ref()
        .is_some_and(|handoff| handoff.completed_at.is_none());
    f.notifications.unavailable.store(false, Ordering::SeqCst);
    drop(initial);
    let restarted = f.gateway()?;
    let recovered = if mutation == Mutation::SkipRecovery {
        0
    } else {
        restarted
            .reconcile_chain_cancellation_handoffs()
            .await
            .map_err(error)?
    };
    let restored = restarted
        .get_chain_status(NAMESPACE, TENANT, &chain_id)
        .await
        .map_err(error)?
        .ok_or_else(|| error("cancelled chain missing after restart"))?;
    let outage_ids = f.notifications.delivery_ids_after(outage_start);
    record(
        report,
        "provider_outage_recovered",
        pending_after_outage
            && recovered == 1
            && restored.status == ChainStatus::Cancelled
            && restored
                .cancellation_handoff
                .as_ref()
                .is_some_and(|handoff| handoff.completed_at.is_some()),
        &json!({
            "pending_after_outage": pending_after_outage,
            "recovered": recovered,
            "attempts": f.notifications.attempts() - outage_start,
            "completed": restored.cancellation_handoff.as_ref().is_some_and(|handoff| handoff.completed_at.is_some()),
        }),
    );
    record(
        report,
        "stable_delivery_id",
        outage_ids.len() == 2 && outage_ids.iter().all(|id| id == &outage_id),
        &json!({"outage_attempts": outage_ids.len(), "same_delivery_id": outage_ids.iter().all(|id| id == &outage_id)}),
    );
    if mutation == Mutation::SkipRecovery {
        restarted
            .reconcile_chain_cancellation_handoffs()
            .await
            .map_err(error)?;
    }

    // The provider has accepted the cancellation, but its response is lost
    // before the gateway can acknowledge the durable handoff. Replacing the
    // gateway and sweeping again sends the same delivery ID; the independently
    // observed receiver must make one effect.
    let ack_start = f.notifications.attempts();
    let effects_before_ack = f.notifications.effects();
    f.notifications.lose_response.store(true, Ordering::SeqCst);
    let ack_gateway = f.gateway()?;
    let ack_chain = f.start(&ack_gateway).await?;
    let ack_cancelled = ack_gateway
        .cancel_chain(NAMESPACE, TENANT, &ack_chain, None, None)
        .await
        .map_err(error)?;
    let ack_id = ack_cancelled
        .cancellation_handoff
        .as_ref()
        .ok_or_else(|| error("terminal cancellation must retain a handoff"))?
        .delivery_id
        .clone();
    drop(ack_gateway);
    let ack_restarted = f.gateway()?;
    let ack_recovered = ack_restarted
        .reconcile_chain_cancellation_handoffs()
        .await
        .map_err(error)?;
    let ack_ids = f.notifications.delivery_ids_after(ack_start);
    let ack_effects = f.notifications.effects() - effects_before_ack;
    let ack_completed = ack_restarted
        .get_chain_status(NAMESPACE, TENANT, &ack_chain)
        .await
        .map_err(error)?
        .and_then(|chain| chain.cancellation_handoff)
        .is_some_and(|handoff| handoff.completed_at.is_some());
    record(
        report,
        "post_effect_transport_loss_one_effect",
        ack_recovered == 1
            && ack_ids.len() == 2
            && ack_ids.iter().all(|id| id == &ack_id)
            && ack_effects == 1
            && ack_completed,
        &json!({
            "recovered": ack_recovered,
            "attempts": ack_ids.len(),
            "same_delivery_id": ack_ids.iter().all(|id| id == &ack_id),
            "effects": ack_effects,
            "completed": ack_completed,
        }),
    );
    record(
        report,
        "transport_failures_observed",
        f.notifications.failures() == 2,
        &json!({"failures": f.notifications.failures()}),
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
            seed: 20_260_909,
            backend: Backend::Memory,
            scenarios: vec![SCENARIO],
        }
    }

    #[tokio::test]
    async fn cancellation_handoff_evidence_replays_and_mutations_fail_gates() {
        let baseline = super::super::run(manifest()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        let replay = super::super::run(manifest()).await.unwrap();
        assert!(replay.same_evidence(&baseline));
        for (mutation, gate) in [
            (Mutation::SkipRecovery, "provider_outage_recovered"),
            (
                Mutation::DisableDownstreamDedup,
                "post_effect_transport_loss_one_effect",
            ),
        ] {
            let mut report = super::super::run(manifest()).await.unwrap();
            report.invariants.clear();
            report.trace.clear();
            run_with(&mut report, mutation).await.unwrap();
            assert!(
                report
                    .invariants
                    .iter()
                    .any(|check| check.name == gate && !check.passed)
            );
            assert!(!evaluation::grade(SCENARIO, &report).passed);
        }
    }
}
