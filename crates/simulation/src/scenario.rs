//! Versioned, replayable semantic scenarios over production gateway boundaries.
//! Logical sequence numbers encode causality. The deadline suite uses explicit
//! virtual time; other suites use real clocks. Volatile IDs never replace checks.

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome};
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    FailureMode, RecordingProvider, SimulationConfig, SimulationError, SimulationHarness,
    StateBackendConfig,
};

mod deadlines;
pub mod evaluation;
mod portfolio;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScenarioManifest {
    pub schema_version: u32,
    pub seed: u64,
    pub backend: Backend,
    pub scenarios: Vec<Scenario>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Memory,
    Redis,
    Postgres,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    GeneratedPolicy,
    Approval,
    TenantDeduplication,
    RetryRecovery,
    EvaluatorIntegrity,
    StateFailure,
    IncidentResponse,
    RefundFulfillment,
    PromptInjection,
    DeadlineSafety,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Invariant {
    pub scenario: Scenario,
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraceEvent {
    pub sequence: usize,
    pub caused_by: Option<usize>,
    pub scenario: Scenario,
    pub operation: String,
    pub observation: String,
    pub provider_calls: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioReport {
    pub schema_version: u32,
    pub manifest: ScenarioManifest,
    pub manifest_sha256: String,
    pub implementation_version: String,
    pub invariants: Vec<Invariant>,
    pub trace: Vec<TraceEvent>,
}

impl ScenarioReport {
    pub fn passed(&self) -> bool {
        !self.invariants.is_empty() && self.invariants.iter().all(|check| check.passed)
    }

    fn check(&mut self, scenario: Scenario, name: &str, passed: bool, detail: impl Into<String>) {
        self.invariants.push(Invariant {
            scenario,
            name: name.into(),
            passed,
            detail: detail.into(),
        });
    }

    fn event(
        &mut self,
        scenario: Scenario,
        operation: &str,
        observation: &str,
        provider_calls: usize,
    ) {
        let sequence = self.trace.len();
        self.trace.push(TraceEvent {
            sequence,
            caused_by: self
                .trace
                .iter()
                .rposition(|event| event.scenario == scenario),
            scenario,
            operation: operation.into(),
            observation: observation.into(),
            provider_calls,
        });
    }

    /// Replay compares the manifest and invariant/semantic evidence, excluding OS timing.
    pub fn same_evidence(&self, previous: &Self) -> bool {
        self.schema_version == previous.schema_version
            && self.manifest == previous.manifest
            && self.manifest_sha256 == previous.manifest_sha256
            && self.invariants == previous.invariants
            && self.trace == previous.trace
    }

    pub fn junit(&self) -> String {
        use std::fmt::Write as _;
        let failures = self.invariants.iter().filter(|check| !check.passed).count();
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite name=\"acteon-scenarios\" tests=\"{}\" failures=\"{failures}\">",
            self.invariants.len()
        );
        for check in &self.invariants {
            let _ = write!(
                xml,
                "<testcase classname=\"{:?}\" name=\"{}\">",
                check.scenario,
                escape_xml(&check.name)
            );
            if !check.passed {
                let _ = write!(
                    xml,
                    "<failure message=\"invariant failed\">{}</failure>",
                    escape_xml(&check.detail)
                );
            }
            xml.push_str("</testcase>");
        }
        xml.push_str("</testsuite>");
        xml
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn backend_config(backend: Backend) -> Result<StateBackendConfig, SimulationError> {
    match backend {
        Backend::Memory => Ok(StateBackendConfig::Memory),
        #[cfg(feature = "redis")]
        Backend::Redis => Ok(StateBackendConfig::Redis {
            url: std::env::var("REDIS_URL")
                .map_err(|_| SimulationError::Configuration("REDIS_URL is required".into()))?,
            prefix: None,
        }),
        #[cfg(feature = "postgres")]
        Backend::Postgres => Ok(StateBackendConfig::Postgres {
            url: std::env::var("DATABASE_URL")
                .map_err(|_| SimulationError::Configuration("DATABASE_URL is required".into()))?,
        }),
        #[allow(unreachable_patterns)]
        _ => Err(SimulationError::Configuration(
            "requested backend feature is not compiled in".into(),
        )),
    }
}

async fn harness(backend: Backend, rules: &str) -> Result<SimulationHarness, SimulationError> {
    SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(2)
            .shared_state(true)
            .state_backend(backend_config(backend)?)
            .add_recording_provider("effect")
            .add_recording_provider("approval-notifications")
            .add_rule_yaml(rules)
            .build(),
    )
    .await
}

fn outcome_name(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "executed",
        ActionOutcome::Deduplicated => "deduplicated",
        ActionOutcome::Suppressed { .. } => "suppressed",
        ActionOutcome::PendingApproval { .. } => "pending_approval",
        ActionOutcome::Failed(_) => "failed",
        _ => "other",
    }
}

fn action(
    seed: u64,
    sequence: usize,
    tenant: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Action {
    let mut action = Action::new("scenario", tenant, "effect", kind, payload);
    action.id = format!("scenario-{seed}-{sequence}-{tenant}").into();
    action
}

/// Run every selected scenario; exceptions become failed invariants, never skips.
pub async fn run(manifest: ScenarioManifest) -> Result<ScenarioReport, SimulationError> {
    if manifest.schema_version != 1 || manifest.scenarios.is_empty() {
        return Err(SimulationError::Configuration(
            "manifest requires schema_version=1 and nonempty scenarios".into(),
        ));
    }
    for (index, scenario) in manifest.scenarios.iter().enumerate() {
        if manifest.scenarios[..index].contains(scenario) {
            return Err(SimulationError::Configuration("duplicate scenario".into()));
        }
    }
    if manifest.backend != Backend::Memory && manifest.scenarios.contains(&Scenario::DeadlineSafety)
    {
        return Err(SimulationError::Configuration(
            "deadline_safety requires the memory backend".into(),
        ));
    }
    let digest = Sha256::digest(serde_json::to_vec(&manifest).expect("manifest serializes"));
    let mut report = ScenarioReport {
        schema_version: 1,
        manifest_sha256: format!("{digest:x}"),
        implementation_version: env!("CARGO_PKG_VERSION").into(),
        manifest,
        invariants: Vec::new(),
        trace: Vec::new(),
    };
    for scenario in report.manifest.scenarios.clone() {
        let result = match scenario {
            Scenario::GeneratedPolicy => policy(&mut report).await,
            Scenario::Approval => approval(&mut report).await,
            Scenario::TenantDeduplication => dedup(&mut report).await,
            Scenario::RetryRecovery => retry(&mut report).await,
            Scenario::EvaluatorIntegrity => evaluator(&mut report).await,
            Scenario::StateFailure => state_failure(&mut report).await,
            Scenario::IncidentResponse => portfolio::incident(&mut report).await,
            Scenario::RefundFulfillment => portfolio::refund(&mut report).await,
            Scenario::PromptInjection => portfolio::injection(&mut report).await,
            Scenario::DeadlineSafety => deadlines::run(&mut report).await,
        };
        if let Err(error) = result {
            report.check(scenario, "scenario completed", false, error.to_string());
        }
    }
    Ok(report)
}

async fn policy(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let scenario = Scenario::GeneratedPolicy;
    let rules = acteon_swarm::acteon::rules::generate_safety_rules(
        "scenario",
        "alice",
        &acteon_swarm::config::SafetyConfig::default(),
    );
    let harness = harness(report.manifest.backend, &rules).await?;
    report.event(
        scenario,
        "backend instantiated",
        harness.state_backend_identity(),
        0,
    );
    for (index, (kind, payload, expected)) in [
        (
            "execute_command",
            serde_json::json!({"command":"rm -rf /tmp/fixture"}),
            "suppressed",
        ),
        (
            "write_file",
            serde_json::json!({"file_path":".ssh/id_rsa"}),
            "suppressed",
        ),
        ("unknown_tool", serde_json::json!({}), "suppressed"),
        (
            "execute_command",
            serde_json::json!({"command":"git push origin main"}),
            "pending_approval",
        ),
        (
            "execute_command",
            serde_json::json!({"command":"cargo check"}),
            "executed",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = harness
            .dispatch_to(
                index % 2,
                &action(report.manifest.seed, index, "alice", kind, payload),
            )
            .await
            .map_err(|e| SimulationError::Dispatch(e.to_string()))?;
        report.check(
            scenario,
            &format!("policy case {index}"),
            outcome_name(&outcome) == expected,
            format!("expected {expected}; observed {}", outcome_name(&outcome)),
        );
        report.event(
            scenario,
            &format!("dispatch case {index}"),
            outcome_name(&outcome),
            harness.provider("effect").unwrap().call_count(),
        );
    }
    report.check(
        scenario,
        "blocked and pending actions produced no effect",
        harness.provider("effect").unwrap().call_count() == 1,
        "only cargo check may reach the effect provider",
    );
    harness.teardown().await
}

async fn dedup(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let scenario = Scenario::TenantDeduplication;
    let harness = harness(report.manifest.backend, "rules:\n  - name: once\n    condition: {field: action.action_type, eq: write}\n    action: {type: deduplicate, ttl_seconds: 3600}\n").await?;
    report.event(
        scenario,
        "backend instantiated",
        harness.state_backend_identity(),
        0,
    );
    let mut rng = rand::rngs::StdRng::seed_from_u64(report.manifest.seed);
    let mut first = action(
        report.manifest.seed,
        0,
        "alice",
        "write",
        serde_json::json!({}),
    );
    first.dedup_key = Some("same-effect-key".into());
    let node = rng.gen_range(0..2);
    let initial = harness
        .dispatch_to(node, &first)
        .await
        .map_err(|e| SimulationError::Dispatch(e.to_string()))?;
    report.check(
        scenario,
        "first dispatch executes",
        matches!(initial, ActionOutcome::Executed(_)),
        outcome_name(&initial),
    );
    report.event(
        scenario,
        &format!("initial dispatch on node {node}"),
        outcome_name(&initial),
        harness.provider("effect").unwrap().call_count(),
    );
    let repeated =
        futures::future::join_all((0..16).map(|index| harness.dispatch_to(index % 2, &first)))
            .await;
    report.check(
        scenario,
        "all duplicate deliveries suppressed across nodes",
        repeated
            .iter()
            .all(|outcome| matches!(outcome, Ok(ActionOutcome::Deduplicated))),
        "16 redeliveries of a completed key",
    );
    let mut other = first.clone();
    other.tenant = "bob".into();
    let isolated = harness
        .dispatch_to(1 - node, &other)
        .await
        .map_err(|e| SimulationError::Dispatch(e.to_string()))?;
    report.check(
        scenario,
        "same key in another tenant executes",
        matches!(isolated, ActionOutcome::Executed(_)),
        outcome_name(&isolated),
    );
    let count = harness.provider("effect").unwrap().call_count();
    report.check(
        scenario,
        "one completed effect per tenant",
        count == 2,
        format!("provider calls: {count}"),
    );
    report.event(
        scenario,
        "duplicate batch and other tenant",
        outcome_name(&isolated),
        count,
    );
    harness.teardown().await
}

async fn approval(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let scenario = Scenario::Approval;
    let harness = harness(report.manifest.backend, "rules:\n  - name: approval\n    condition: {field: action.action_type, eq: publish}\n    action: {type: request_approval, notify_provider: approval-notifications, timeout_seconds: 3600}\n").await?;
    let outcome = harness
        .dispatch(&action(
            report.manifest.seed,
            0,
            "alice",
            "publish",
            serde_json::json!({}),
        ))
        .await
        .map_err(|e| SimulationError::Dispatch(e.to_string()))?;
    let calls = harness.provider("effect").unwrap().call_count();
    report.check(
        scenario,
        "pending approval has no effect",
        calls == 0,
        format!("provider calls: {calls}"),
    );
    report.event(scenario, "request approval", outcome_name(&outcome), calls);
    let ActionOutcome::PendingApproval {
        approval_id,
        approve_url,
        ..
    } = outcome
    else {
        return Err(SimulationError::Gateway(
            "approval was not requested".into(),
        ));
    };
    let url = reqwest::Url::parse(&approve_url)
        .map_err(|e| SimulationError::Configuration(e.to_string()))?;
    let query: std::collections::HashMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let signature = query
        .get("sig")
        .ok_or_else(|| SimulationError::Gateway("missing approval signature".into()))?;
    let expires = query
        .get("expires_at")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| SimulationError::Gateway("missing approval expiry".into()))?;
    let gateway = harness.node(1).unwrap().gateway();
    let wrong_tenant = gateway
        .execute_approval(
            "scenario",
            "bob",
            &approval_id,
            signature,
            expires,
            query.get("kid").map(String::as_str),
        )
        .await;
    report.check(
        scenario,
        "approval cannot cross tenants",
        wrong_tenant.is_err(),
        "signature is scoped to tenant",
    );
    let approved = gateway
        .execute_approval(
            "scenario",
            "alice",
            &approval_id,
            signature,
            expires,
            query.get("kid").map(String::as_str),
        )
        .await
        .map_err(|e| SimulationError::Dispatch(e.to_string()))?;
    let replay = gateway
        .execute_approval(
            "scenario",
            "alice",
            &approval_id,
            signature,
            expires,
            query.get("kid").map(String::as_str),
        )
        .await;
    let calls = harness.provider("effect").unwrap().call_count();
    report.check(
        scenario,
        "approval executes once and rejects replay",
        matches!(approved, ActionOutcome::Executed(_)) && replay.is_err() && calls == 1,
        format!("provider calls: {calls}"),
    );
    report.event(
        scenario,
        "approve and replay",
        outcome_name(&approved),
        calls,
    );
    harness.teardown().await
}

async fn retry(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let scenario = Scenario::RetryRecovery;
    let failures = rand::rngs::StdRng::seed_from_u64(report.manifest.seed).gen_range(1..=3);
    let provider = RecordingProvider::new("effect")
        .with_seed(report.manifest.seed)
        .with_failure_mode(FailureMode::FirstN(failures));
    let dlq = Arc::new(acteon_executor::DeadLetterQueue::new());
    let executor = acteon_executor::ActionExecutor::with_dlq(
        acteon_executor::ExecutorConfig {
            max_retries: 3,
            retry_strategy: acteon_executor::RetryStrategy::Constant {
                delay: Duration::ZERO,
            },
            ..Default::default()
        },
        dlq.clone(),
    );
    let outcome = executor
        .execute(
            &action(
                report.manifest.seed,
                0,
                "alice",
                "retry",
                serde_json::json!({}),
            ),
            &provider,
        )
        .await;
    report.check(
        scenario,
        "transient failures recover within budget",
        matches!(outcome, ActionOutcome::Executed(_))
            && provider.call_count() == failures + 1
            && dlq.is_empty(),
        format!(
            "injected failures: {failures}; attempts: {}",
            provider.call_count()
        ),
    );
    report.event(
        scenario,
        "transient outage then recovery",
        outcome_name(&outcome),
        provider.call_count(),
    );
    let permanent = RecordingProvider::new("effect").with_failure_mode(FailureMode::Always);
    let outcome = executor
        .execute(
            &action(
                report.manifest.seed,
                1,
                "alice",
                "retry",
                serde_json::json!({}),
            ),
            &permanent,
        )
        .await;
    report.check(
        scenario,
        "exhausted retries retained in DLQ",
        matches!(outcome, ActionOutcome::Failed(_))
            && permanent.call_count() == 4
            && dlq.len() == 1,
        format!(
            "attempts: {}; DLQ entries: {}",
            permanent.call_count(),
            dlq.len()
        ),
    );
    report.event(
        scenario,
        "permanent outage",
        outcome_name(&outcome),
        permanent.call_count(),
    );
    Ok(())
}

async fn state_failure(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let scenario = Scenario::StateFailure;
    let harness = harness(report.manifest.backend, "rules:\n  - name: counter\n    condition: {field: action.action_type, eq: limited}\n    action: {type: throttle, max_count: 2, window_seconds: 3600}\n").await?;
    let state = harness.node(0).unwrap().gateway().state_store();
    let key = acteon_state::StateKey::new(
        "scenario",
        "alice",
        acteon_state::KeyKind::RateLimit,
        "counter",
    );
    state
        .set(&key, "invalid-counter", None)
        .await
        .map_err(|e| SimulationError::Gateway(e.to_string()))?;
    let action = action(
        report.manifest.seed,
        0,
        "alice",
        "limited",
        serde_json::json!({}),
    );
    let refused = harness.dispatch(&action).await;
    let calls = harness.provider("effect").unwrap().call_count();
    report.check(
        scenario,
        "unreadable rate limit state cannot authorize an effect",
        refused.is_err() && calls == 0,
        "counter parsing error must fail closed",
    );
    report.event(
        scenario,
        "dispatch with unreadable counter",
        if refused.is_err() {
            "refused"
        } else {
            "unexpected execution"
        },
        calls,
    );
    state
        .delete(&key)
        .await
        .map_err(|e| SimulationError::Gateway(e.to_string()))?;
    let restored = harness
        .dispatch(&action)
        .await
        .map_err(|e| SimulationError::Dispatch(e.to_string()))?;
    let calls = harness.provider("effect").unwrap().call_count();
    report.check(
        scenario,
        "dispatch recovers after counter repair",
        matches!(restored, ActionOutcome::Executed(_)) && calls == 1,
        format!("provider calls: {calls}"),
    );
    report.event(
        scenario,
        "repair counter and retry",
        outcome_name(&restored),
        calls,
    );
    harness.teardown().await
}

async fn evaluator(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let scenario = Scenario::EvaluatorIntegrity;
    for (index, payload) in [
        "",
        "SCORE: NaN",
        "SCORE: 2",
        "PASS: 0/0",
        "SCORE: 1\nWARNINGS: -1",
        "SCORE: 1\nSCORE: 0",
    ]
    .iter()
    .enumerate()
    {
        let config = acteon_swarm::config::EvalHarnessConfig {
            enabled: true,
            program: Some("printf".into()),
            args: vec!["%s".into(), (*payload).into()],
            command: String::new(),
            ..Default::default()
        };
        let refused =
            acteon_swarm::orchestrator::eval::run_eval_harness(&config, std::path::Path::new("."))
                .await
                .is_err();
        report.check(
            scenario,
            &format!("reject invalid score {index}"),
            refused,
            "invalid evidence must not become a passing score",
        );
    }
    report.event(
        scenario,
        "invalid evaluator evidence",
        "validated by production parser",
        0,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn memory_suite_passes_and_replays_semantic_evidence() {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 42,
            backend: Backend::Memory,
            scenarios: vec![
                Scenario::GeneratedPolicy,
                Scenario::Approval,
                Scenario::TenantDeduplication,
                Scenario::RetryRecovery,
                Scenario::EvaluatorIntegrity,
                Scenario::StateFailure,
            ],
        };
        let first = run(manifest.clone()).await.unwrap();
        assert!(
            first.passed(),
            "{}",
            serde_json::to_string_pretty(&first).unwrap()
        );
        let replay = run(manifest).await.unwrap();
        assert!(replay.same_evidence(&first));
        assert!(first.junit().contains("failures=\"0\""));
    }
}
