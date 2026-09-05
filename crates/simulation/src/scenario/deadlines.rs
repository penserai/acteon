//! Explicit virtual-time evidence over real gateway/executor/memory code.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use acteon_core::{Action, ActionOutcome, ProviderResponse};
use acteon_executor::{ExecutorConfig, RetryStrategy};
use acteon_gateway::{Gateway, GatewayBuilder, GatewayError};
use acteon_provider::{Provider, ProviderError};
use acteon_rules::RuleFrontend;
use acteon_state::{DistributedLock, KeyKind, StateKey, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};
use parking_lot::Mutex;
use serde_json::json;

use super::evaluation::derived_seed;
use super::{Backend, Scenario, ScenarioReport};
use crate::scheduler::DeterministicScheduler;
use crate::{ActionOutcomeExt, RecordingProvider, SimulationError};

pub(super) const CLOCK_DESCRIPTION: &str =
    "deadline_safety=manual UTC epoch 2023-11-14T22:13:20Z; other scenarios=wall_clock";
const SCENARIO: Scenario = Scenario::DeadlineSafety;
const EPOCH: i64 = 1_700_000_000;

fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(error.to_string())
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(EPOCH, 0).expect("fixed epoch"),
    ))
}

fn action(kind: &str, sequence: usize) -> Action {
    let mut action = super::action(0, sequence, "alice", kind, json!({}));
    action.created_at = chrono::DateTime::from_timestamp(EPOCH, 0).expect("fixed epoch");
    action
}

fn gateway(
    clock: Arc<ManualClock>,
    store: Arc<MemoryStateStore>,
    lock: Arc<MemoryDistributedLock>,
    provider: Arc<dyn acteon_provider::DynProvider>,
    rules: &str,
    config: ExecutorConfig,
) -> Result<Gateway, SimulationError> {
    let rules = acteon_rules_yaml::YamlFrontend
        .parse(rules)
        .map_err(error)?;
    GatewayBuilder::new()
        .clock(clock)
        .state(store)
        .lock(lock)
        .rules(rules)
        .executor_config(config)
        .provider(provider)
        .provider(Arc::new(RecordingProvider::new("approval-notifications")))
        .approval_secret(b"deadline-fixture-signing-key-32bytes".to_vec())
        .build()
        .map_err(error)
}

fn check(report: &mut ScenarioReport, name: &str, passed: bool, detail: impl Into<String>) {
    report.check(SCENARIO, name, passed, detail);
}

fn trace(
    report: &mut ScenarioReport,
    operation: &str,
    observation: impl serde::Serialize,
    calls: usize,
) {
    report.event(
        SCENARIO,
        operation,
        &serde_json::to_string(&observation).expect("trace serializes"),
        calls,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    FrozenApprovalClock,
    ExtendedExecutionTimeout,
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}

async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    if report.manifest.backend != Backend::Memory {
        return Err(SimulationError::Configuration(
            "deadline_safety requires the memory backend; remote TTL clocks are not virtualized"
                .into(),
        ));
    }
    trace(report, "clock domain", CLOCK_DESCRIPTION, 0);
    dedup(report).await?;
    approval(report, mutation).await?;
    leases(report).await?;
    timeouts(report, mutation)?;
    retry(report)?;
    Ok(())
}

async fn dedup(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let lock = Arc::new(MemoryDistributedLock::with_clock(clock.clone()));
    let provider = Arc::new(RecordingProvider::new("effect"));
    let rules = "rules:\n  - name: dedup\n    condition: {field: action.action_type, eq: alert}\n    action: {type: deduplicate, ttl_seconds: 10}\n";
    let first = gateway(
        clock.clone(),
        store.clone(),
        lock.clone(),
        provider.clone(),
        rules,
        ExecutorConfig::default(),
    )?;
    let second = gateway(
        clock.clone(),
        store,
        lock,
        provider.clone(),
        rules,
        ExecutorConfig::default(),
    )?;
    let initial = first
        .dispatch(action("alert", 0).with_dedup_key("deadline-alert"), None)
        .await
        .map_err(error)?;
    clock
        .advance_to(Duration::from_millis(9_999))
        .map_err(error)?;
    let before = second
        .dispatch(action("alert", 1).with_dedup_key("deadline-alert"), None)
        .await
        .map_err(error)?;
    clock.advance_to(Duration::from_secs(10)).map_err(error)?;
    let at = second
        .dispatch(action("alert", 2).with_dedup_key("deadline-alert"), None)
        .await
        .map_err(error)?;
    let observations = [
        super::outcome_name(&initial),
        super::outcome_name(&before),
        super::outcome_name(&at),
    ];
    check(
        report,
        "dedup_boundary",
        observations == ["executed", "deduplicated", "executed"] && provider.call_count() == 2,
        format!(
            "at_ms=[0,9999,10000], outcomes={observations:?}, effects={}",
            provider.call_count()
        ),
    );
    trace(
        report,
        "dedup expiry across two gateways",
        observations,
        provider.call_count(),
    );
    first.shutdown().await;
    second.shutdown().await;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn approval(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let mut evidence = Vec::new();
    // Retain each approval without a TTL so the gateway's authority is tested
    // independently of lazy eviction (as needed for eventual-TTL backends).
    for (created_ms, at_ms) in [
        (0, 1_999),
        (0, 2_000),
        (0, 2_001),
        (500, 1_999),
        (500, 2_000),
        (500, 2_001),
    ] {
        let clock = clock();
        clock
            .advance_to(Duration::from_millis(created_ms))
            .map_err(error)?;
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let lock = Arc::new(MemoryDistributedLock::with_clock(clock.clone()));
        let provider = Arc::new(RecordingProvider::new("effect"));
        let rules = "rules:\n  - name: approval\n    condition: {field: action.action_type, eq: remediate}\n    action: {type: request_approval, notify_provider: approval-notifications, timeout_seconds: 2}\n";
        let gateway_clock = if mutation == Mutation::FrozenApprovalClock {
            let frozen = self::clock();
            frozen
                .advance_to(Duration::from_millis(created_ms))
                .map_err(error)?;
            frozen
        } else {
            clock.clone()
        };
        let gateway = gateway(
            gateway_clock,
            store.clone(),
            lock,
            provider.clone(),
            rules,
            ExecutorConfig::default(),
        )?;
        let pending = gateway
            .dispatch(action("remediate", 0), None)
            .await
            .map_err(error)?;
        let ActionOutcome::PendingApproval {
            approval_id,
            approve_url,
            ..
        } = pending
        else {
            return Err(error("approval did not pend"));
        };
        let key = StateKey::new("scenario", "alice", KeyKind::Approval, &approval_id);
        let raw = store
            .get(&key)
            .await
            .map_err(error)?
            .ok_or_else(|| error("approval missing"))?;
        store.set(&key, &raw, None).await.map_err(error)?;
        let url = reqwest::Url::parse(&approve_url).map_err(error)?;
        let query: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let signature = query.get("sig").ok_or_else(|| error("missing signature"))?;
        let signed_deadline = query
            .get("expires_at")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| error("missing expiration"))?;
        clock
            .advance_to(Duration::from_millis(created_ms + at_ms))
            .map_err(error)?;
        let result = gateway
            .execute_approval(
                "scenario",
                "alice",
                &approval_id,
                signature,
                signed_deadline,
                query.get("kid").map(String::as_str),
            )
            .await;
        let executed = matches!(result, Ok(ActionOutcome::Executed(_)));
        let expired = matches!(result, Err(GatewayError::ApprovalNotFound));
        // Reject has the same exclusive signature boundary and must not claim it.
        let reject_expired = if at_ms >= 2_000 {
            matches!(
                gateway
                    .reject_approval(
                        "scenario",
                        "alice",
                        &approval_id,
                        signature,
                        signed_deadline,
                        query.get("kid").map(String::as_str)
                    )
                    .await,
                Err(GatewayError::ApprovalNotFound)
            )
        } else {
            true
        };
        evidence.push((
            created_ms,
            at_ms,
            executed,
            expired,
            reject_expired,
            provider.call_count(),
        ));
        gateway.shutdown().await;
    }
    check(
        report,
        "approval_boundary",
        evidence
            == [
                (0, 1_999, true, false, true, 1),
                (0, 2_000, false, true, true, 0),
                (0, 2_001, false, true, true, 0),
                (500, 1_999, true, false, true, 1),
                (500, 2_000, false, true, true, 0),
                (500, 2_001, false, true, true, 0),
            ],
        serde_json::to_string(&evidence).expect("approval evidence serializes"),
    );
    trace(
        report,
        "approval expiry with retained records",
        &evidence,
        evidence.iter().map(|v| v.5).sum(),
    );
    Ok(())
}

async fn leases(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let clock = clock();
    let lock = MemoryDistributedLock::with_clock(clock.clone());
    let old = lock
        .try_acquire("lease", Duration::from_secs(1))
        .await
        .map_err(error)?
        .ok_or_else(|| error("initial lease missing"))?;
    clock
        .advance_to(Duration::from_millis(999))
        .map_err(error)?;
    let held_before = old.is_held().await.map_err(error)?;
    clock.advance_to(Duration::from_secs(1)).map_err(error)?;
    let expired = !old.is_held().await.map_err(error)?;
    let renewal_denied = old.extend(Duration::from_secs(10)).await.is_err();
    let successor = lock
        .try_acquire("lease", Duration::from_secs(1))
        .await
        .map_err(error)?
        .ok_or_else(|| error("successor lease missing"))?;
    old.release().await.map_err(error)?;
    let successor_held = successor.is_held().await.map_err(error)?;
    let mut scheduler = DeterministicScheduler::<()>::new(clock.clone(), 100);
    let wait = scheduler
        .run(
            lock.acquire("lease", Duration::from_secs(1), Duration::from_secs(1)),
            |()| {},
        )
        .map_err(error)?;
    let timeout = matches!(wait, Err(acteon_state::StateError::Timeout(_)));
    let elapsed = clock.monotonic();
    let new_owner = lock
        .try_acquire("lease", Duration::from_secs(1))
        .await
        .map_err(error)?;
    let passed = held_before
        && expired
        && renewal_denied
        && successor_held
        && timeout
        && elapsed == Duration::from_secs(2)
        && new_owner.is_some();
    check(
        report,
        "lease_boundary",
        passed,
        format!(
            "held_before={held_before}, expired={expired}, renewal_denied={renewal_denied}, successor_held={successor_held}, waiter_timed_out={timeout}, elapsed_ms={}",
            elapsed.as_millis()
        ),
    );
    trace(report, "lease waiter schedule", scheduler.trace(), 0);
    successor.release().await.map_err(error)?;
    if let Some(owner) = new_owner {
        owner.release().await.map_err(error)?;
    }
    Ok(())
}

#[derive(Default)]
struct EffectLedger {
    attempts: Vec<u128>,
    effects: Vec<u128>,
}

struct TimedProvider {
    clock: Arc<ManualClock>,
    delay: Duration,
    down: Arc<AtomicBool>,
    ledger: Arc<Mutex<EffectLedger>>,
}

impl Provider for TimedProvider {
    fn name(&self) -> &'static str {
        "effect"
    }
    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.ledger
            .lock()
            .attempts
            .push(self.clock.monotonic().as_millis());
        if self.down.load(Ordering::SeqCst) {
            return Err(ProviderError::Connection("scheduled outage".into()));
        }
        self.clock.sleep(self.delay).await;
        self.ledger
            .lock()
            .effects
            .push(self.clock.monotonic().as_millis());
        Ok(ProviderResponse::success(json!({"completed": true})))
    }
    fn health_check(&self) -> impl std::future::Future<Output = Result<(), ProviderError>> + Send {
        std::future::ready(Ok(()))
    }
}

fn timeouts(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let mut evidence = Vec::new();
    for delay in [99, 100, 101] {
        let clock = clock();
        let ledger = Arc::new(Mutex::new(EffectLedger::default()));
        let provider = Arc::new(TimedProvider {
            clock: clock.clone(),
            delay: Duration::from_millis(delay),
            down: Arc::new(AtomicBool::new(false)),
            ledger: ledger.clone(),
        });
        let config = ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_millis(
                if mutation == Mutation::ExtendedExecutionTimeout {
                    1_000
                } else {
                    100
                },
            ),
            ..Default::default()
        };
        let gateway = gateway(
            clock.clone(),
            Arc::new(MemoryStateStore::with_clock(clock.clone())),
            Arc::new(MemoryDistributedLock::with_clock(clock.clone())),
            provider,
            "rules: []",
            config,
        )?;
        let mut scheduler = DeterministicScheduler::<()>::new(clock.clone(), 20);
        let result = scheduler
            .run(gateway.dispatch(action("timed", 0), None), |()| {})
            .map_err(error)?
            .map_err(error)?;
        let timed_out = matches!(&result, ActionOutcome::Failed(error) if error.code == "TIMEOUT" && error.attempts == 1);
        evidence.push((
            delay,
            result.is_executed(),
            timed_out,
            ledger.lock().effects.clone(),
            clock.monotonic().as_millis(),
            clock.pending_timers(),
        ));
        trace(
            report,
            "execution deadline schedule",
            scheduler.trace(),
            ledger.lock().attempts.len(),
        );
    }
    check(
        report,
        "timeout_boundary",
        evidence
            == [
                (99, true, false, vec![99], 99, 0),
                (100, false, true, vec![], 100, 0),
                (101, false, true, vec![], 100, 0),
            ],
        serde_json::to_string(&evidence).expect("timeout evidence serializes"),
    );
    trace(report, "exclusive execution deadlines", evidence, 3);
    Ok(())
}

fn retry(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let clock = clock();
    let down = Arc::new(AtomicBool::new(false));
    let ledger = Arc::new(Mutex::new(EffectLedger::default()));
    let provider = Arc::new(TimedProvider {
        clock: clock.clone(),
        delay: Duration::ZERO,
        down: down.clone(),
        ledger: ledger.clone(),
    });
    let config = ExecutorConfig {
        max_retries: 2,
        retry_strategy: RetryStrategy::Constant {
            delay: Duration::from_millis(100),
        },
        execution_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let gateway = gateway(
        clock.clone(),
        Arc::new(MemoryStateStore::with_clock(clock.clone())),
        Arc::new(MemoryDistributedLock::with_clock(clock.clone())),
        provider,
        "rules: []",
        config,
    )?;
    let recovery_ms = 101 + derived_seed(report.manifest.seed, 0, "outage_recovery") % 99;
    let mut scheduler = DeterministicScheduler::new(clock.clone(), 50);
    scheduler
        .schedule(0, "outage begins", true)
        .map_err(error)?;
    scheduler
        .schedule(recovery_ms, "outage ends", false)
        .map_err(error)?;
    let result = scheduler
        .run(gateway.dispatch(action("retry", 0), None), |value| {
            down.store(value, Ordering::SeqCst);
        })
        .map_err(error)?
        .map_err(error)?;
    let ledger = ledger.lock();
    check(
        report,
        "retry_schedule",
        result.is_executed()
            && ledger.attempts == [0, 100, 200]
            && ledger.effects == [200]
            && scheduler.pending_events() == 0
            && clock.pending_timers() == 0,
        format!(
            "recovery_ms={recovery_ms}, attempts_ms={:?}, effects_ms={:?}",
            ledger.attempts, ledger.effects
        ),
    );
    trace(
        report,
        "scheduled and consumed outage",
        scheduler.trace(),
        ledger.attempts.len(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{ScenarioManifest, evaluation};
    use super::*;

    #[tokio::test]
    async fn deadline_evidence_replays_and_mutations_fail_safety_gates() {
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
        for (mutation, name) in [
            (Mutation::FrozenApprovalClock, "approval_boundary"),
            (Mutation::ExtendedExecutionTimeout, "timeout_boundary"),
        ] {
            let mut report = super::super::run(baseline.manifest.clone()).await.unwrap();
            report.invariants.clear();
            report.trace.clear();
            run_with(&mut report, mutation).await.unwrap();
            assert!(
                report
                    .invariants
                    .iter()
                    .any(|check| check.name == name && !check.passed)
            );
            let score = evaluation::grade(SCENARIO, &report);
            assert!(!score.passed);
            assert!(score.gates.iter().any(|gate| !gate.passed));
        }
    }
}
