//! Real Laya inference followed by deterministic Acteon routing.
//!
//! Start the local Laya service first:
//!   cd examples/neural-observability-detector
//!   docker compose up -d --build --wait
//!
//! Run from the repository root:
//!   cargo run -p acteon-simulation --features bus,redis --example `neural_observability_simulation` -- --write-results

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_bus::{
    BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, StartOffset, StreamCheckpointConfig,
    StreamCheckpointCoordinator, StreamDeliveryError, StreamOutboxConfig, StreamOutboxDelivery,
    StreamOutboxDispatchResult, StreamOutboxDispatcher, StreamOutboxEntry, StreamOutboxMetrics,
    stream_checkpoint_key,
};
use acteon_core::chain::{ChainConfig, ChainStatus, ChainStepConfig, DispatchStepConfig};
use acteon_core::{Action, ActionOutcome, ProviderResponse, Topic};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_llm::{GovernedModelProvider, GovernedModelProviderConfig, VerifiedModelLock};
use acteon_provider::{Provider, ProviderError};
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::{ActionOutcomeExt, RecordingProvider};
use acteon_state::StateStore;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_state_redis::{RedisConfig, RedisStateStore};
use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[path = "neural_observability/windowing.rs"]
mod windowing;

#[path = "neural_observability/checkpoint.rs"]
mod checkpoint;

use checkpoint::{
    WindowCheckpoint, is_recovery_record, open_windows, source_positions, stream_positions,
    window_key, window_outputs,
};
use windowing::{
    CorrelatedWindow, EventTimeCorrelator, IngestDisposition, SCHEMA_VERSION, SignalSource,
    SourcePosition, TelemetryEvent, WindowingStats,
};

type AnyError = Box<dyn std::error::Error + Send + Sync>;

const MODEL: &str = "typed-decisions";
const NAMESPACE: &str = "observability";
const TENANT: &str = "acme";

#[derive(Debug, Clone, Deserialize)]
struct Fixture {
    name: String,
    window_id: String,
    window_start: DateTime<Utc>,
    expected_route: String,
    metrics: Value,
    traces: Value,
    logs: Value,
}

#[derive(Debug, Serialize)]
struct HealthIdentity {
    loaded: Vec<String>,
    revisions: BTreeMap<String, String>,
    device: String,
}

#[derive(Debug)]
struct LayaCall {
    response: Value,
    provider_evidence: BTreeMap<String, String>,
    model_request_ms: f64,
    wall_ms: f64,
}

#[derive(Debug, Deserialize)]
struct LayaEnvelope {
    answers: BTreeMap<String, Value>,
    model: String,
    routing: LayaRouting,
    usage: LayaUsage,
}

#[derive(Debug, Deserialize)]
struct LayaRouting {
    model: String,
}

#[derive(Debug, Deserialize)]
struct LayaUsage {
    output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignalSummary {
    source: String,
    condition: String,
    condition_confidence: f64,
    evidence_probability: f64,
    impact_score: f64,
    available: bool,
    model_request_ms: f64,
    wall_ms: f64,
    raw_response: Value,
    provider_evidence: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FusionSummary {
    incident_kind: String,
    answer_confidence: f64,
    probabilities: BTreeMap<String, f64>,
    correlated_probability: f64,
    impact_score: f64,
    model_request_ms: f64,
    wall_ms: f64,
    raw_response: Value,
    provider_evidence: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrialReport {
    name: String,
    window_id: String,
    expected_route: String,
    admitted_route: String,
    evidence_strength: f64,
    uncertainty: f64,
    source_positions: BTreeMap<String, SourcePosition>,
    signals: Vec<SignalSummary>,
    fusion: FusionSummary,
    acteon_outcome: String,
    passed: bool,
}

#[derive(Debug, Serialize)]
struct AggregateReport {
    model_calls: usize,
    total_model_request_ms: f64,
    p50_model_request_ms: f64,
    p95_model_request_ms: f64,
    diagnostic_calls: usize,
    on_call_notifications: usize,
    investigator_calls: usize,
    duplicate_dispatches_prevented: usize,
}

#[derive(Debug, Serialize)]
struct RecoveryReport {
    checkpoint_writes: usize,
    restored_generation: u64,
    pre_crash_records: usize,
    replayed_records_deduplicated: usize,
    committed_offsets: BTreeMap<String, SourcePosition>,
    final_consumer_lag: BTreeMap<String, i64>,
}

#[derive(Debug, Serialize)]
struct SimulationReport {
    generated_at: String,
    model: String,
    health: HealthIdentity,
    governance: VerifiedModelLock,
    windowing: WindowingStats,
    recovery: RecoveryReport,
    trials: Vec<TrialReport>,
    aggregate: AggregateReport,
    delivery: DeliveryReport,
}

struct StreamReplay {
    windows: Vec<CorrelatedWindow>,
    stats: WindowingStats,
    recovery: RecoveryReport,
    checkpoints: WindowCheckpoint,
}

struct LayaClient {
    health_http: reqwest::Client,
    base_url: String,
    api_key: String,
    gateway: Gateway,
    governance: VerifiedModelLock,
    calls: AtomicUsize,
}

impl LayaClient {
    fn new(root: &Path, governance: VerifiedModelLock) -> Result<Self, AnyError> {
        let base_url =
            std::env::var("LAYA_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".to_owned());
        let api_key =
            std::env::var("LAYA_API_KEY").unwrap_or_else(|_| "acteon-laya-demo".to_owned());
        let mut builder = GatewayBuilder::new()
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(90),
                ..Default::default()
            })
            .state(Arc::new(MemoryStateStore::new()))
            .lock(Arc::new(MemoryDistributedLock::new()));
        for name in ["metrics", "traces", "logs", "fusion"] {
            builder = builder.provider(Arc::new(GovernedModelProvider::new(
                GovernedModelProviderConfig {
                    name: format!("laya-{name}"),
                    endpoint: format!("{base_url}/v1/systemone"),
                    health_endpoint: format!("{base_url}/health"),
                    lock_file: root.join("model.lock.json"),
                    contracts_root: root.to_path_buf(),
                    response_contract: "response_schema".into(),
                    request_contract: Some(name.into()),
                    request_contract_field: Some("questions".into()),
                    model_field: Some("model".into()),
                    bearer_token: Some(api_key.clone()),
                    timeout: Duration::from_secs(90),
                    max_response_bytes: NonZeroUsize::new(1024 * 1024).unwrap(),
                    verify_identity_each_call: true,
                },
            )?));
        }
        Ok(Self {
            health_http: reqwest::Client::builder()
                .timeout(Duration::from_secs(90))
                .build()?,
            base_url,
            api_key,
            gateway: builder.build()?,
            governance,
            calls: AtomicUsize::new(0),
        })
    }

    async fn health(&self) -> Result<HealthIdentity, AnyError> {
        let value: Value = self
            .health_http
            .get(format!("{}/health", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let loaded = value
            .get("loaded")
            .and_then(Value::as_array)
            .ok_or_else(|| error("Laya health response has no loaded checkpoint list"))?
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error("Laya health loaded entry is not a string"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let revisions = serde_json::from_value(
            value
                .get("revisions")
                .cloned()
                .ok_or_else(|| error("Laya health response has no revisions"))?,
        )?;
        let device = string_at(&value, &["device"])?;
        Ok(HealthIdentity {
            loaded,
            revisions,
            device,
        })
    }

    async fn evaluate(
        &self,
        name: &str,
        state: Value,
        questions: &Value,
    ) -> Result<LayaCall, AnyError> {
        let started = std::time::Instant::now();
        let outcome = self
            .gateway
            .dispatch(
                Action::new(
                    NAMESPACE,
                    TENANT,
                    format!("laya-{name}"),
                    "model.evaluate",
                    json!({"state": state}),
                ),
                None,
            )
            .await?;
        let ActionOutcome::Executed(response) = outcome else {
            return Err(error(format!(
                "governed inference was not executed: {outcome:?}"
            )));
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        for (header, expected) in [
            (
                "acteon-model-lock-digest",
                self.governance.lock_digest.as_str(),
            ),
            (
                "acteon-model-revision",
                self.governance.policy.model.revision.as_str(),
            ),
            ("acteon-model-request-contract", name),
            ("acteon-model-response-contract", "response_schema"),
        ] {
            if response.headers.get(header).map(String::as_str) != Some(expected) {
                return Err(error(format!(
                    "governed inference evidence mismatch: {header}"
                )));
            }
        }
        let elapsed = response
            .headers
            .get("acteon-model-elapsed-ms")
            .ok_or_else(|| error("provider omitted request timing"))?
            .parse::<f64>()?;
        let envelope: LayaEnvelope = serde_json::from_value(response.body.clone())?;
        if envelope.routing.model != MODEL
            || envelope.usage.output_tokens != 0
            || envelope.answers.len() != questions.as_object().map_or(0, serde_json::Map::len)
            || envelope.model.is_empty()
        {
            return Err(error("typed Laya response identity or usage check failed"));
        }
        validate_laya_response(&response.body, questions)?;
        Ok(LayaCall {
            response: response.body,
            model_request_ms: elapsed,
            // Whole milliseconds remain exact across checkpoint JSON round trips.
            wall_ms: (started.elapsed().as_secs_f64() * 1000.0).round(),
            provider_evidence: response.headers.into_iter().collect(),
        })
    }
}

fn error(message: impl Into<String>) -> AnyError {
    std::io::Error::other(message.into()).into()
}

fn object_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Map<String, Value>, AnyError> {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .ok_or_else(|| error(format!("missing JSON field {}", path.join("."))))?;
    }
    current
        .as_object()
        .ok_or_else(|| error(format!("{} is not an object", path.join("."))))
}

fn string_at(value: &Value, path: &[&str]) -> Result<String, AnyError> {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .ok_or_else(|| error(format!("missing JSON field {}", path.join("."))))?;
    }
    current
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| error(format!("{} is not a string", path.join("."))))
}

fn number_at(value: &Value, path: &[&str]) -> Result<f64, AnyError> {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .ok_or_else(|| error(format!("missing JSON field {}", path.join("."))))?;
    }
    let number = current
        .as_f64()
        .ok_or_else(|| error(format!("{} is not numeric", path.join("."))))?;
    if !number.is_finite() {
        return Err(error(format!("{} is not finite", path.join("."))));
    }
    Ok(number)
}

fn validate_probability(value: f64, field: &str) -> Result<(), AnyError> {
    if !(0.0..=1.0).contains(&value) {
        return Err(error(format!(
            "{field} probability {value} is outside 0..1"
        )));
    }
    Ok(())
}

fn validate_laya_response(response: &Value, questions: &Value) -> Result<(), AnyError> {
    let root = response
        .as_object()
        .ok_or_else(|| error("Laya response is not an object"))?;
    let root_keys = root.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected_root = BTreeSet::from(["answers", "model", "routing", "usage"]);
    if root_keys != expected_root {
        return Err(error(format!(
            "unexpected Laya response fields: {root_keys:?}"
        )));
    }
    if string_at(response, &["routing", "model"])? != MODEL {
        return Err(error("Laya routed the request to an unexpected checkpoint"));
    }
    if number_at(response, &["usage", "output_tokens"])? != 0.0 {
        return Err(error(
            "non-autoregressive Laya response reported output tokens",
        ));
    }

    let question_map = questions
        .as_object()
        .ok_or_else(|| error("question set is not an object"))?;
    let answers = object_at(response, &["answers"])?;
    if question_map.keys().collect::<BTreeSet<_>>() != answers.keys().collect::<BTreeSet<_>>() {
        return Err(error(
            "Laya answer IDs do not match the requested question IDs",
        ));
    }

    for (id, question) in question_map {
        let answer = answers
            .get(id)
            .and_then(Value::as_object)
            .ok_or_else(|| error(format!("answer {id} is not an object")))?;
        let question_type = question
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error(format!("question {id} has no type")))?;
        if answer.get("type").and_then(Value::as_str) != Some(question_type) {
            return Err(error(format!("answer {id} has the wrong type")));
        }
        for field in ["confidence", "answer_confidence"] {
            let value = answer
                .get(field)
                .and_then(Value::as_f64)
                .ok_or_else(|| error(format!("answer {id} has no numeric {field}")))?;
            validate_probability(value, &format!("{id}.{field}"))?;
        }
        let act_probability = answer
            .get("action")
            .and_then(|action| action.get("act_probability"))
            .and_then(Value::as_f64)
            .ok_or_else(|| error(format!("answer {id} has no action probability")))?;
        validate_probability(act_probability, &format!("{id}.action.act_probability"))?;

        match question_type {
            "choice" => validate_choice(id, question, answer)?,
            "score" => validate_score(id, question, answer)?,
            "noul" => {
                let value = answer
                    .get("noul")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| error(format!("answer {id} has no noul value")))?;
                validate_probability(value, &format!("{id}.noul"))?;
            }
            other => return Err(error(format!("unsupported question type {other}"))),
        }
    }
    Ok(())
}

fn validate_choice(
    id: &str,
    question: &Value,
    answer: &Map<String, Value>,
) -> Result<(), AnyError> {
    let criteria = question
        .get("criteria")
        .and_then(Value::as_object)
        .ok_or_else(|| error(format!("choice question {id} has no criteria")))?;
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| error(format!("answer {id} has no choice")))?;
    if !criteria.contains_key(choice) {
        return Err(error(format!(
            "answer {id} returned unknown choice {choice}"
        )));
    }
    validate_distribution(id, answer, criteria.keys().map(String::as_str))
}

fn validate_score(id: &str, question: &Value, answer: &Map<String, Value>) -> Result<(), AnyError> {
    let criteria = question
        .get("criteria")
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("score question {id} has no criteria")))?;
    let score = answer
        .get("score")
        .and_then(Value::as_f64)
        .ok_or_else(|| error(format!("answer {id} has no score")))?;
    let last_level = u32::try_from(criteria.len() - 1)?;
    if !score.is_finite() || !(0.0..=f64::from(last_level)).contains(&score) {
        return Err(error(format!(
            "answer {id} score {score} is outside its scale"
        )));
    }
    let labels = (0..criteria.len())
        .map(|index| index.to_string())
        .collect::<Vec<_>>();
    validate_distribution(id, answer, labels.iter().map(String::as_str))
}

fn validate_distribution<'a>(
    id: &str,
    answer: &Map<String, Value>,
    expected_labels: impl Iterator<Item = &'a str>,
) -> Result<(), AnyError> {
    let probabilities = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| error(format!("answer {id} has no probability distribution")))?;
    let expected = expected_labels.collect::<BTreeSet<_>>();
    let actual = probabilities
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(error(format!(
            "answer {id} probability labels do not match criteria"
        )));
    }
    let mut sum = 0.0;
    for (label, probability) in probabilities {
        let value = probability
            .as_f64()
            .ok_or_else(|| error(format!("answer {id}.{label} is not numeric")))?;
        validate_probability(value, &format!("{id}.{label}"))?;
        sum += value;
    }
    if (sum - 1.0).abs() > 0.002 {
        return Err(error(format!("answer {id} probabilities sum to {sum}")));
    }
    Ok(())
}

fn load_fixture(path: &Path) -> Result<Fixture, AnyError> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn parse_rules(yaml: &str) -> Result<Vec<Rule>, AnyError> {
    let frontend = YamlFrontend;
    Ok(acteon_rules::RuleFrontend::parse(&frontend, yaml)?)
}

struct ActeonSimulation {
    gateway: Gateway,
    routing_gateway: Arc<Gateway>,
    diagnostics: Arc<RecordingProvider>,
    on_call: Arc<RecordingProvider>,
    investigator: Arc<RecordingProvider>,
    notification_intake: Arc<RecordingProvider>,
}

impl ActeonSimulation {
    #[cfg(test)]
    fn build(rule_yaml: &str) -> Result<Self, AnyError> {
        Self::build_with_store(rule_yaml, Arc::new(MemoryStateStore::new()))
    }

    fn build_with_store(rule_yaml: &str, store: Arc<dyn StateStore>) -> Result<Self, AnyError> {
        let diagnostics = Arc::new(RecordingProvider::new("diagnostics"));
        let on_call = Arc::new(RecordingProvider::new("on-call"));
        let investigator = Arc::new(RecordingProvider::new("investigator"));
        let notification_intake = Arc::new(RecordingProvider::new("notification-intake"));
        let verdict_audit = Arc::new(RecordingProvider::new("verdict-audit"));
        let chain = ChainConfig::new("observability-incident")
            .with_step(ChainStepConfig::new(
                "capture-diagnostics",
                "diagnostics",
                "capture_snapshot",
                json!({
                    "incident_key": "{{origin.payload.incident_key}}",
                    "evidence": "{{origin.payload.evidence_refs}}"
                }),
            ))
            .with_step(ChainStepConfig::new_dispatch(
                "notify-on-call",
                DispatchStepConfig {
                    provider: "notification-intake".into(),
                    action_type: "incident.notify".into(),
                    dedup_key: Some("{{origin.payload.incident_key}}:notify".into()),
                    inherit_metadata: true,
                },
                json!({
                    "incident_key": "{{origin.payload.incident_key}}",
                    "kind": "{{origin.payload.kind}}"
                }),
            ))
            .with_timeout(30);
        let providers: Vec<Arc<dyn acteon_provider::DynProvider>> = vec![
            Arc::clone(&diagnostics) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&on_call) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&investigator) as Arc<dyn acteon_provider::DynProvider>,
            verdict_audit as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&notification_intake) as Arc<dyn acteon_provider::DynProvider>,
        ];
        let mut builder = GatewayBuilder::new()
            .state(Arc::new(MemoryStateStore::new()))
            .lock(Arc::new(MemoryDistributedLock::new()))
            .rules(parse_rules(rule_yaml)?)
            .chain(chain)
            .completed_chain_ttl(Duration::from_secs(3_600));
        for provider in providers {
            builder = builder.provider(provider);
        }
        let routing_gateway = Arc::new(builder.build()?);
        let admission_rule = fs::read_to_string(example_root().join("rules/admission.yaml"))?;
        let admission_gateway = GatewayBuilder::new()
            .executor_config(ExecutorConfig {
                max_retries: 0,
                ..Default::default()
            })
            .state(store)
            .lock(Arc::new(MemoryDistributedLock::new()))
            .rules(parse_rules(&admission_rule)?)
            .provider(Arc::new(VerdictDispatchProvider {
                gateway: Arc::clone(&routing_gateway),
            }))
            .build()?;
        Ok(Self {
            gateway: admission_gateway,
            routing_gateway,
            diagnostics,
            on_call,
            investigator,
            notification_intake,
        })
    }

    async fn dispatch(&self, action: Action) -> Result<String, AnyError> {
        let key = action
            .dedup_key
            .clone()
            .ok_or_else(|| error("verdict has no stable deduplication key"))?;
        let delivery = Action::new(
            NAMESPACE,
            TENANT,
            "verdict-dispatch",
            "detector.deliver",
            serde_json::to_value(action)?,
        )
        .with_dedup_key(key);
        match self.gateway.dispatch(delivery, None).await? {
            ActionOutcome::Executed(response) => string_at(&response.body, &["acteon_outcome"]),
            ActionOutcome::Deduplicated => Ok("deduplicated by Acteon".into()),
            outcome => Err(error(format!("verdict admission failed: {outcome:?}"))),
        }
    }
}

struct VerdictDispatchProvider {
    gateway: Arc<Gateway>,
}
impl Provider for VerdictDispatchProvider {
    fn name(&self) -> &'static str {
        "verdict-dispatch"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let verdict = serde_json::from_value(action.payload.clone())
            .map_err(|err| ProviderError::Serialization(err.to_string()))?;
        let outcome = Box::pin(dispatch_verdict(&self.gateway, verdict))
            .await
            .map_err(|err| ProviderError::ExecutionFailed(err.to_string()))?;
        Ok(ProviderResponse::success(
            json!({"acteon_outcome": outcome}),
        ))
    }
    fn health_check(&self) -> impl std::future::Future<Output = Result<(), ProviderError>> + Send {
        std::future::ready(Ok(()))
    }
}

async fn dispatch_verdict(gateway: &Gateway, action: Action) -> Result<String, AnyError> {
    let outcome = gateway.dispatch(action, None).await?;
    match &outcome {
        ActionOutcome::Deduplicated => Ok("deduplicated by Acteon".into()),
        ActionOutcome::Suppressed { rule } => {
            outcome.assert_suppressed();
            Ok(format!("suppressed by {rule}"))
        }
        ActionOutcome::Rerouted { new_provider, .. } => {
            if new_provider != "investigator" {
                return Err(error(format!(
                    "expected investigator reroute, got {new_provider}"
                )));
            }
            Ok(format!("rerouted to {new_provider}"))
        }
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            ..
        } => {
            if chain_name != "observability-incident" {
                return Err(error(format!("unexpected chain {chain_name}")));
            }
            Box::pin(gateway.advance_chain(NAMESPACE, TENANT, chain_id)).await?;
            Box::pin(gateway.advance_chain(NAMESPACE, TENANT, chain_id)).await?;
            let state = gateway
                .get_chain_status(NAMESPACE, TENANT, chain_id)
                .await?
                .ok_or_else(|| error("incident chain state disappeared"))?;
            if state.status != ChainStatus::Completed {
                return Err(error(format!(
                    "incident chain did not complete: {:?}",
                    state.status
                )));
            }
            Ok("completed observability-incident chain".to_owned())
        }
        other => Err(error(format!("unexpected Acteon outcome: {other:?}"))),
    }
}

fn signal_summary(
    source: &str,
    call: LayaCall,
    evidence_question: &str,
    available: bool,
) -> Result<SignalSummary, AnyError> {
    Ok(SignalSummary {
        source: source.to_owned(),
        condition: string_at(&call.response, &["answers", "condition", "choice"])?,
        condition_confidence: number_at(
            &call.response,
            &["answers", "condition", "answer_confidence"],
        )?,
        evidence_probability: number_at(&call.response, &["answers", evidence_question, "noul"])?,
        impact_score: number_at(&call.response, &["answers", "impact", "score"])?,
        available,
        model_request_ms: call.model_request_ms,
        wall_ms: call.wall_ms,
        raw_response: call.response,
        provider_evidence: call.provider_evidence,
    })
}

fn fusion_summary(call: LayaCall) -> Result<FusionSummary, AnyError> {
    let probabilities = serde_json::from_value(
        call.response
            .pointer("/answers/incident_kind/probabilities")
            .cloned()
            .ok_or_else(|| error("fusion answer has no probabilities"))?,
    )?;
    Ok(FusionSummary {
        incident_kind: string_at(&call.response, &["answers", "incident_kind", "choice"])?,
        answer_confidence: number_at(
            &call.response,
            &["answers", "incident_kind", "answer_confidence"],
        )?,
        probabilities,
        correlated_probability: number_at(
            &call.response,
            &["answers", "correlated_incident", "noul"],
        )?,
        impact_score: number_at(&call.response, &["answers", "impact", "score"])?,
        model_request_ms: call.model_request_ms,
        wall_ms: call.wall_ms,
        raw_response: call.response,
        provider_evidence: call.provider_evidence,
    })
}

fn choose_policy(signals: &[SignalSummary], fusion: &FusionSummary) -> (&'static str, f64, f64) {
    let metrics = &signals[0];
    let traces = &signals[1];
    let logs = &signals[2];
    let healthy = metrics.condition == "healthy"
        && traces.condition == "healthy"
        && matches!(logs.condition.as_str(), "healthy" | "unrelated_noise");
    let corroborated_pool_exhaustion = metrics.condition == "db_pool_pressure"
        && traces.condition == "database_wait"
        && logs.condition == "pool_timeout"
        && fusion.incident_kind == "db_pool_exhaustion";
    let missing_signal = signals.iter().any(|signal| !signal.available);
    let non_healthy_labels = signals
        .iter()
        .filter(|signal| signal.condition != "healthy")
        .map(|signal| signal.condition.as_str())
        .collect::<BTreeSet<_>>();
    let disagreement = non_healthy_labels.len() > 1;
    let evidence_strength = (metrics.evidence_probability
        + traces.evidence_probability
        + logs.evidence_probability
        + fusion.correlated_probability)
        / 4.0;
    let uncertainty = if missing_signal {
        0.75
    } else if disagreement && !corroborated_pool_exhaustion {
        0.65
    } else {
        1.0 - fusion.answer_confidence
    };

    if healthy {
        ("suppress", evidence_strength, uncertainty)
    } else if corroborated_pool_exhaustion {
        ("incident", evidence_strength, uncertainty)
    } else {
        ("investigate", evidence_strength, uncertainty)
    }
}

fn fusion_state(fixture: &Fixture, signals: &[SignalSummary]) -> Value {
    let typed_signals = signals
        .iter()
        .map(|signal| {
            (
                signal.source.clone(),
                signal
                    .raw_response
                    .get("answers")
                    .cloned()
                    .unwrap_or(Value::Null),
            )
        })
        .collect::<Map<_, _>>();
    let availability = signals
        .iter()
        .map(|signal| (signal.source.clone(), Value::Bool(signal.available)))
        .collect::<Map<_, _>>();
    json!({
        "scenario": fixture.name,
        "service": "checkout-api",
        "environment": "prod",
        "signals": typed_signals,
        "available": availability
    })
}

fn evidence_refs(fixture: &Fixture) -> Vec<String> {
    [&fixture.metrics, &fixture.traces, &fixture.logs]
        .iter()
        .filter_map(|source| source.get("evidence_refs").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn outcome_action(
    fixture: &Fixture,
    route: &str,
    evidence_strength: f64,
    uncertainty: f64,
    fusion: &FusionSummary,
    governance: &VerifiedModelLock,
) -> Action {
    Action::new(
        NAMESPACE,
        TENANT,
        "verdict-audit",
        "detector.verdict",
        json!({
            "schema_version": 1,
            "incident_key": fixture.window_id,
            "service": "checkout-api",
            "environment": "prod",
            "kind": fusion.incident_kind,
            "policy_band": route,
            "evidence_strength": evidence_strength,
            "uncertainty": uncertainty,
            "evidence_refs": evidence_refs(fixture),
            "model": {
                "repository": governance.policy.model.repository,
                "revision": governance.policy.model.revision,
                "checkpoint": governance.policy.model.name,
                "lock_digest": governance.lock_digest,
                "question_set": "fusion@1"
            }
        }),
    )
    .with_dedup_key(fixture.window_id.clone())
}

#[derive(Debug, Serialize)]
struct DeliveryReport {
    state_backend: String,
    attempts: u64,
    accepted_outputs: u64,
    retries_scheduled: u64,
    worker_restarts: usize,
    deduplicated_redeliveries: usize,
    pending_outputs: usize,
    retained_dead_letters: usize,
    dead_letter_reason: String,
    model_calls_repeated_on_retry: usize,
    pending_window_outputs: usize,
    recovered_decisions: usize,
}

struct GatewayDelivery {
    acteon: Arc<ActeonSimulation>,
    outcomes: Arc<Mutex<BTreeMap<String, String>>>,
    lose_incident_ack: AtomicBool,
    deduplicated: Arc<AtomicUsize>,
}

#[async_trait]
impl StreamOutboxDelivery<Action> for GatewayDelivery {
    async fn deliver(&self, entry: &StreamOutboxEntry<Action>) -> Result<(), StreamDeliveryError> {
        let action = &entry.payload;
        if action.namespace.as_str() != NAMESPACE
            || action.tenant.as_str() != TENANT
            || action.provider.as_str() != "verdict-audit"
            || action.action_type.as_str() != "detector.verdict"
            || action.payload.get("schema_version") != Some(&json!(1))
            || action.dedup_key.as_deref() != Some(entry.idempotency_key.as_str())
            || !matches!(
                action.payload.get("policy_band").and_then(Value::as_str),
                Some("suppress" | "incident" | "investigate")
            )
        {
            return Err(StreamDeliveryError::Permanent(
                "invalid verdict envelope or idempotency key".into(),
            ));
        }
        let outcome = Box::pin(self.acteon.dispatch(action.clone()))
            .await
            .map_err(|err| StreamDeliveryError::Retryable(err.to_string()))?;
        if outcome == "deduplicated by Acteon" {
            self.deduplicated.fetch_add(1, Ordering::SeqCst);
        } else {
            self.outcomes
                .lock()
                .expect("observer results")
                .insert(entry.idempotency_key.clone(), outcome);
        }
        if action.payload["policy_band"] == "incident"
            && self.lose_incident_ack.swap(false, Ordering::SeqCst)
        {
            // The receiver has completed the chain; lose only its acknowledgement.
            return Err(StreamDeliveryError::Retryable(
                "injected acknowledgement loss after accepted incident".into(),
            ));
        }
        Ok(())
    }
}

async fn open_verdicts(
    redis: &RedisConfig,
    key: acteon_state::StateKey,
) -> Result<StreamCheckpointCoordinator<Vec<TrialReport>, Action>, AnyError> {
    Ok(StreamCheckpointCoordinator::initialize(
        Arc::new(RedisStateStore::new(redis)?),
        key,
        Vec::new(),
        StreamCheckpointConfig::default(),
    )
    .await?)
}

#[allow(clippy::too_many_lines)]
async fn deliver_verdicts(
    redis: &RedisConfig,
    key: acteon_state::StateKey,
    acteon: Arc<ActeonSimulation>,
    reports: &mut [TrialReport],
) -> Result<DeliveryReport, AnyError> {
    let config = StreamOutboxConfig {
        initial_backoff_ms: 100,
        max_backoff_ms: 100,
        ..Default::default()
    };
    let outcomes = Arc::new(Mutex::new(BTreeMap::new()));
    let deduplicated = Arc::new(AtomicUsize::new(0));
    let mut producer = open_verdicts(redis, key.clone()).await?;
    producer
        .checkpoint(
            producer.snapshot().state().clone(),
            [],
            [StreamOutboxEntry {
                idempotency_key: "invalid-verdict".into(),
                created_at: Utc::now(),
                payload: Action::new(
                    NAMESPACE,
                    TENANT,
                    "verdict-audit",
                    "detector.verdict",
                    json!({}),
                )
                .with_dedup_key("invalid-verdict"),
            }],
        )
        .await?;
    drop(producer);
    let receiver = GatewayDelivery {
        acteon: Arc::clone(&acteon),
        outcomes: Arc::clone(&outcomes),
        lose_incident_ack: AtomicBool::new(true),
        deduplicated: Arc::clone(&deduplicated),
    };
    let mut dispatcher = StreamOutboxDispatcher::initialize(
        open_verdicts(redis, key.clone()).await?,
        "delivery-before-restart",
        config.clone(),
    )
    .await?;
    let retry_at = loop {
        match dispatcher.dispatch_once(&receiver).await? {
            StreamOutboxDispatchResult::RetryScheduled {
                next_attempt_at, ..
            } => break next_attempt_at,
            StreamOutboxDispatchResult::Idle => {
                return Err(error("expected acknowledgement-loss retry before idle"));
            }
            _ => {}
        }
    };
    println!("outbox: accepted incident, lost acknowledgement, persisted retry; replacing worker");
    drop(dispatcher);
    drop(receiver);
    // New pool, coordinator, dispatcher, and delivery adapter; only the receiver
    // gateway and external observer survive, as in a remote service deployment.
    let recovered = open_verdicts(redis, key).await?;
    if serde_json::to_value(recovered.snapshot().state())? != serde_json::to_value(&*reports)? {
        return Err(error("replacement worker lost cached decisions"));
    }
    let mut dispatcher =
        StreamOutboxDispatcher::initialize(recovered, "delivery-after-restart", config).await?;
    let restored = dispatcher.metrics().await?;
    if restored.counters.retries_scheduled != 1 || restored.pending == 0 {
        return Err(error(
            "replacement worker failed to recover the pending retry",
        ));
    }
    let receiver = GatewayDelivery {
        acteon,
        outcomes: Arc::clone(&outcomes),
        lose_incident_ack: AtomicBool::new(false),
        deduplicated: Arc::clone(&deduplicated),
    };
    let wait = retry_at
        .signed_duration_since(Utc::now())
        .to_std()
        .unwrap_or_default();
    tokio::time::sleep(wait + Duration::from_millis(10)).await;
    for _ in 0..reports.len() + 2 {
        if dispatcher.dispatch_once(&receiver).await? == StreamOutboxDispatchResult::Idle {
            break;
        }
    }
    let metrics = dispatcher.metrics().await?;
    if metrics.pending != 0
        || metrics.counters.delivered != 4
        || metrics.counters.attempts != 6
        || metrics.counters.retries_scheduled != 1
        || metrics.dead_letters != 1
    {
        return Err(error(format!(
            "unexpected managed delivery metrics: {metrics:?}"
        )));
    }
    let letter = &dispatcher.dead_letters()[0];
    if letter.entry.idempotency_key != "invalid-verdict" {
        return Err(error("unexpected dead letter"));
    }
    let reason = letter.last_error.clone();
    for report in reports {
        report.acteon_outcome = outcomes
            .lock()
            .expect("observer results")
            .get(&report.window_id)
            .cloned()
            .ok_or_else(|| error("delivered verdict missing from receiver observer"))?;
    }
    let result = delivery_report(&metrics, deduplicated.load(Ordering::SeqCst), reason);
    dispatcher.discard_dead_letter("invalid-verdict").await?;
    println!(
        "outbox: {} attempts, {} accepted, {} retry, {} deduplicated redelivery, {} inspected dead letter, {} pending",
        result.attempts,
        result.accepted_outputs,
        result.retries_scheduled,
        result.deduplicated_redeliveries,
        result.retained_dead_letters,
        result.pending_outputs
    );
    Ok(result)
}

fn delivery_report(
    metrics: &StreamOutboxMetrics,
    duplicates: usize,
    reason: String,
) -> DeliveryReport {
    DeliveryReport {
        state_backend: "redis".into(),
        attempts: metrics.counters.attempts,
        accepted_outputs: metrics.counters.delivered,
        retries_scheduled: metrics.counters.retries_scheduled,
        worker_restarts: 1,
        deduplicated_redeliveries: duplicates,
        pending_outputs: metrics.pending,
        retained_dead_letters: metrics.dead_letters,
        dead_letter_reason: reason,
        model_calls_repeated_on_retry: 0,
        pending_window_outputs: 0,
        recovered_decisions: 4,
    }
}

fn percentile(sorted: &[f64], numerator: usize, denominator: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) * numerator).div_ceil(denominator);
    sorted[index]
}

fn markdown(report: &SimulationReport) -> String {
    let mut output = String::new();
    output.push_str("# Neural observability simulation results\n\n");
    let _ = write!(
        output,
        "Laya `{}` ran on `{}` at revision `{}`. Governance lock `{}` approved six runtime packages, five checkpoint artifacts, four question sets, and a response schema. Kafka supplied {} accepted source records across {} event-time windows and the pipeline rejected {} duplicates or recovery redeliveries. The runner restored checkpoint generation {} after an injected pre-commit crash. All {} neural calls went through governed providers and real HTTP inference. Redis persisted checkpoints, delivery state, and admission deduplication; the routing gateway used memory for chain state and recording providers for controlled side effects.\n\n",
        report.model,
        report.health.device,
        report
            .health
            .revisions
            .get(MODEL)
            .map_or("unknown", String::as_str),
        report.governance.lock_digest,
        report.windowing.accepted_records,
        report.windowing.complete_windows + report.windowing.incomplete_windows,
        report.windowing.duplicate_records + report.recovery.replayed_records_deduplicated,
        report.recovery.restored_generation,
        report.aggregate.model_calls
    );
    output.push_str(
        "| Trial | Signal decisions | Raw fusion | Acteon outcome | Model HTTP elapsed | Result |\n",
    );
    output.push_str("|---|---|---|---|---:|---|\n");
    for trial in &report.trials {
        let signals = trial
            .signals
            .iter()
            .map(|signal| {
                format!(
                    "{}: {} ({:.2})",
                    signal.source, signal.condition, signal.condition_confidence
                )
            })
            .collect::<Vec<_>>()
            .join("<br>");
        let inference = trial
            .signals
            .iter()
            .map(|signal| signal.model_request_ms)
            .sum::<f64>()
            + trial.fusion.model_request_ms;
        let _ = writeln!(
            output,
            "| {} | {} | {} ({:.2}) | {} | {:.0} ms | {} |",
            trial.name,
            signals,
            trial.fusion.incident_kind,
            trial.fusion.answer_confidence,
            trial.acteon_outcome,
            inference,
            if trial.passed { "PASS" } else { "FAIL" }
        );
    }
    output.push_str("\n## Aggregate\n\n");
    let _ = write!(
        output,
        "- Governed runtime packages / model artifacts / locked contracts: **{} / {} / {}**\n- Kafka source records accepted: **{}**\n- Kafka duplicates and redeliveries rejected: **{}**\n- Recovery redeliveries deduplicated after restart: **{}**\n- Window checkpoints persisted before offset commits: **{}**\n- Final Kafka consumer lag: **{}**\n- Event-time windows completed: **{}**\n- Model calls: **{}**\n- Total model HTTP elapsed: **{:.0} ms**\n- Per-call p50 / p95: **{:.0} ms / {:.0} ms**\n- Incident chains: **{}** diagnostics capture and **{}** on-call notification\n- Bounded investigations: **{}**\n- Duplicate incident dispatches prevented by Acteon admission: **{}**\n\n",
        report.governance.policy.runtime.len(),
        report.governance.policy.artifacts.len(),
        report.governance.policy.contracts.len(),
        report.windowing.accepted_records,
        report.windowing.duplicate_records + report.recovery.replayed_records_deduplicated,
        report.recovery.replayed_records_deduplicated,
        report.recovery.checkpoint_writes,
        report.recovery.final_consumer_lag.values().sum::<i64>(),
        report.windowing.complete_windows,
        report.aggregate.model_calls,
        report.aggregate.total_model_request_ms,
        report.aggregate.p50_model_request_ms,
        report.aggregate.p95_model_request_ms,
        report.aggregate.diagnostic_calls,
        report.aggregate.on_call_notifications,
        report.aggregate.investigator_calls,
        report.aggregate.duplicate_dispatches_prevented
    );
    output.push_str("## Managed delivery recovery\n\n");
    let _ = write!(
        output,
        "| Observation | Result |\n|---|---:|\n| State backend | {} |\n| Cached decisions recovered | {} |\n| Delivery attempts | {} |\n| Accepted verdicts | {} |\n| Persisted retries | {} |\n| Replaced delivery workers | {} |\n| Redeliveries deduplicated by Acteon | {} |\n| Invalid verdicts retained for inspection | {} |\n| Pending window / verdict outputs | {} / {} |\n| Model calls repeated during retry | {} |\n\nDead-letter diagnostic: `{}`. The probe is inspected and explicitly discarded after measurement.\n\nTimings measure model HTTP request and response validation through the governed provider. They are not the server-only inference timings used in the previous report. Full wall times also include gateway dispatch and runtime identity checks.\n\n",
        report.delivery.state_backend,
        report.delivery.recovered_decisions,
        report.delivery.attempts,
        report.delivery.accepted_outputs,
        report.delivery.retries_scheduled,
        report.delivery.worker_restarts,
        report.delivery.deduplicated_redeliveries,
        report.delivery.retained_dead_letters,
        report.delivery.pending_window_outputs,
        report.delivery.pending_outputs,
        report.delivery.model_calls_repeated_on_retry,
        report.delivery.dead_letter_reason,
    );
    output.push_str("## Interpretation\n\n");
    output.push_str("The retry fault occurs after the receiver completes dispatch. A deduplicating admission rule precedes the verdict-routing gateway; the notification chain step re-enters gateway rules. This verifies post-acceptance acknowledgement loss, not an arbitrary receiver crash during execution. Acteon's deduplication rule claims its key before executing its provider, so an interrupted or failed nested dispatch requires separate operational reconciliation.\n\n");
    output.push_str("Laya separated the first-stage signals, including the log-only noise case. Its low-confidence raw fusion choice still selected a non-healthy incident for healthy inputs. The deterministic corroboration gate prevented those raw false positives from reaching a provider. This is the intended safety property: neural decisions contribute bounded evidence, while Acteon policy controls side effects. These fixture results are integration evidence, not a detector-quality benchmark or a calibration claim.\n");
    output
}

fn example_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/neural-observability-detector")
}

fn source_features(fixture: &Fixture, source: SignalSource) -> &Value {
    match source {
        SignalSource::Metrics => &fixture.metrics,
        SignalSource::Traces => &fixture.traces,
        SignalSource::Logs => &fixture.logs,
    }
}

fn canonical_model_state(source: SignalSource, features: &Value) -> Result<Value, AnyError> {
    let object = features
        .as_object()
        .ok_or_else(|| error(format!("{} features are not an object", source.as_str())))?;
    let field_order: &[&str] = match source {
        SignalSource::Metrics => &[
            "context",
            "latency_p95_ms",
            "latency_baseline_ms",
            "error_rate",
            "request_rate_per_second",
            "request_rate_baseline",
            "db_pool_utilization",
            "deployment_age_seconds",
            "evidence_refs",
        ],
        SignalSource::Traces => &[
            "context",
            "slow_request_fraction",
            "database_wait_fraction",
            "dominant_span",
            "evidence_refs",
        ],
        SignalSource::Logs => &[
            "context",
            "error_count",
            "pool_timeout_count",
            "warning_count",
            "top_template",
            "evidence_refs",
        ],
    };
    let allowed = field_order
        .iter()
        .copied()
        .chain(["available"])
        .collect::<BTreeSet<_>>();
    let unknown = object
        .keys()
        .map(String::as_str)
        .filter(|field| !allowed.contains(field))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(error(format!(
            "{} features contain unknown fields: {unknown:?}",
            source.as_str()
        )));
    }
    if !object.contains_key("context") || !object.contains_key("evidence_refs") {
        return Err(error(format!(
            "{} features require context and evidence_refs",
            source.as_str()
        )));
    }
    Ok(Value::Array(
        field_order
            .iter()
            .filter_map(|field| {
                object
                    .get(*field)
                    .map(|value| json!({"field": field, "value": value}))
            })
            .collect(),
    ))
}

fn telemetry_event(fixture: &Fixture, source: SignalSource) -> TelemetryEvent {
    let source_delay = match source {
        SignalSource::Metrics => 10,
        SignalSource::Traces => 20,
        SignalSource::Logs => 30,
    };
    let observed_at = fixture.window_start + ChronoDuration::seconds(source_delay);
    let features = source_features(fixture, source).clone();
    TelemetryEvent {
        schema_version: SCHEMA_VERSION,
        event_id: format!("{}:{}", fixture.name, source.as_str()),
        observed_at,
        ingested_at: observed_at + ChronoDuration::seconds(1),
        window_id: fixture.window_id.clone(),
        tenant: TENANT.to_owned(),
        environment: "prod".to_owned(),
        service: "checkout-api".to_owned(),
        deployment_revision: "checkout-v42".to_owned(),
        source,
        available: features
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        features,
    }
}

fn remember_source_position(
    message: &BusMessage,
    offsets: &mut BTreeMap<SignalSource, SourcePosition>,
) -> Result<(SignalSource, SourcePosition), AnyError> {
    let source = serde_json::from_value::<TelemetryEvent>(message.payload.clone())?.source;
    let position = SourcePosition {
        topic: message.topic.clone(),
        partition: message
            .partition
            .ok_or_else(|| error("Kafka record omitted its partition"))?,
        offset: message
            .offset
            .ok_or_else(|| error("Kafka record omitted its offset"))?,
    };
    if let Some(previous) = offsets.get(&source) {
        if previous.topic != position.topic || previous.partition != position.partition {
            return Err(error(format!(
                "{} source moved between Kafka partitions during the simulation",
                source.as_str()
            )));
        }
        if previous.offset >= position.offset {
            return Ok((source, position));
        }
    }
    offsets.insert(source, position.clone());
    Ok((source, position))
}

#[allow(clippy::too_many_lines)]
async fn kafka_stream_replay(
    fixtures: &[Fixture],
    redis: &RedisConfig,
) -> Result<StreamReplay, AnyError> {
    let bootstrap =
        std::env::var("ACTEON_KAFKA_BOOTSTRAP").unwrap_or_else(|_| "127.0.0.1:19092".to_owned());
    let config = KafkaBusConfig {
        bootstrap_servers: bootstrap,
        client_id: "neural-observability-simulation".to_owned(),
        produce_timeout_ms: 8_000,
        ..KafkaBusConfig::default()
    };
    let backend: Arc<dyn BusBackend> = KafkaBackend::new(&config)?;
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let topics = SignalSource::ALL
        .into_iter()
        .map(|source| {
            let mut topic = Topic::new(format!("{}-{run_id}", source.as_str()), NAMESPACE, TENANT);
            topic.partitions = 1;
            topic.replication_factor = 1;
            (source, topic)
        })
        .collect::<BTreeMap<_, _>>();

    for topic in topics.values() {
        backend.create_topic(topic).await?;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let consumer_groups = SignalSource::ALL
        .into_iter()
        .map(|source| {
            (
                source,
                format!("neural-detector-{}-{run_id}", source.as_str()),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let replay = async {
        for source in SignalSource::ALL {
            let topic = topics[&source].kafka_topic_name();
            for fixture in fixtures {
                let event = telemetry_event(fixture, source);
                backend
                    .produce(
                        BusMessage::new(topic.clone(), serde_json::to_value(&event)?)
                            .with_key(event.correlation_key())
                            .with_header("schema", "observability.telemetry.v1"),
                    )
                    .await?;
                // Test event-ID deduplication before its retention watermark can
                // prune the original ID. Kafka preserves this per-partition order.
                if source == SignalSource::Metrics && fixture.window_id == fixtures[0].window_id {
                    backend.produce(BusMessage::new(topic.clone(), serde_json::to_value(&event)?)
                        .with_key(event.correlation_key()).with_header("schema", "observability.telemetry.v1")).await?;
                }
            }
        }

        let expected_records = fixtures.len() * SignalSource::ALL.len() + 1;
        let pre_crash_records = 5;

        // Phase 1 persists all correlator state and ready outputs, then exits
        // without acknowledging Kafka. This is the injected crash boundary.
        {
            let mut streams = Vec::new();
            for source in SignalSource::ALL {
                streams.push(
                    backend
                        .subscribe(
                            &topics[&source].kafka_topic_name(),
                            &consumer_groups[&source],
                            StartOffset::Earliest,
                        )
                        .await?,
                );
            }
            let mut messages = futures::stream::select_all(streams);
            let mut correlator = EventTimeCorrelator::new(
                ChronoDuration::minutes(1),
                ChronoDuration::seconds(15),
            )?;
            let mut ready_windows = Vec::new();
            let mut source_offsets = BTreeMap::new();
            for _ in 0..pre_crash_records {
                let next = tokio::time::timeout(Duration::from_secs(20), messages.next())
                    .await
                    .map_err(|_| error("timed out before the injected Kafka restart"))?
                    .ok_or_else(|| error("observability Kafka stream ended before restart"))??;
                remember_source_position(&next, &mut source_offsets)?;
                let result = correlator.ingest(next)?;
                if result.disposition == IngestDisposition::Late {
                    return Err(error("fixture stream unexpectedly produced a late record"));
                }
                ready_windows.extend(result.emitted);
            }
            let mut checkpoints = open_windows(redis, correlator.snapshot()).await?;
            checkpoints.checkpoint(correlator.snapshot(), stream_positions(&source_offsets, &consumer_groups), window_outputs(ready_windows)).await?;
            // Drop the entire Redis pool and coordinator before opening a replacement.

        }

        // The old consumers disappear without a commit. A replacement process
        // restores generation 1 and receives the uncommitted prefix again.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let empty = EventTimeCorrelator::new(ChronoDuration::minutes(1), ChronoDuration::seconds(15))?;
        let mut checkpoints = open_windows(redis, empty.snapshot()).await?;
        let restored_generation = checkpoints.snapshot().generation();
        let crash_offsets = source_positions(checkpoints.snapshot().positions())?;
        let mut correlator = EventTimeCorrelator::restore(checkpoints.snapshot().state().clone())?;
        let mut windows = checkpoints.snapshot().pending_outputs().iter().map(|entry| entry.payload.clone()).collect::<Vec<_>>();
        let existing_window_keys = checkpoints.snapshot().pending_outputs().iter().map(|entry| entry.idempotency_key.clone()).collect::<BTreeSet<_>>();
        let mut source_offsets = crash_offsets.clone();
        let mut recovery_redeliveries = 0;
        {
            let mut streams = Vec::new();
            for source in SignalSource::ALL {
                streams.push(
                    backend
                        .subscribe(
                            &topics[&source].kafka_topic_name(),
                            &consumer_groups[&source],
                            StartOffset::Earliest,
                        )
                        .await?,
                );
            }
            let mut messages = futures::stream::select_all(streams);
            for _ in 0..expected_records {
                let next = tokio::time::timeout(Duration::from_secs(20), messages.next())
                    .await
                    .map_err(|_| error("timed out consuming observability Kafka records"))?
                    .ok_or_else(|| error("observability Kafka stream ended early"))??;
                let (source, position) = remember_source_position(&next, &mut source_offsets)?;
                let is_recovery_redelivery = is_recovery_record(source, &position, &crash_offsets);
                if is_recovery_redelivery {
                    // The durable checkpoint already represents this broker
                    // position, even if event-ID retention has since advanced.
                    recovery_redeliveries += 1;
                    continue;
                }
                let result = correlator.ingest(next)?;
                if result.disposition == IngestDisposition::Late {
                    return Err(error("fixture stream unexpectedly produced a late record"));
                }
                windows.extend(result.emitted);
            }
        }

        windows.extend(correlator.finish()?);
        windows.sort_by_key(|window| window.starts_at);
        let stats = correlator.stats().clone();
        if stats.accepted_records != fixtures.len() * SignalSource::ALL.len()
            || stats.duplicate_records != 1
            || stats.late_records != 0
            || stats.complete_windows != fixtures.len()
            || stats.incomplete_windows != 0
        {
            return Err(error(format!("unexpected windowing stats: {stats:?}")));
        }
        if recovery_redeliveries != pre_crash_records {
            return Err(error(format!(
                "expected {pre_crash_records} recovery redeliveries, observed {recovery_redeliveries}"
            )));
        }

        // The stream is dropped before the out-of-band batch commit. Generation
        // 2 is persisted in Redis first; only then does each source offset advance.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let committed = checkpoints.checkpoint_then_commit_bus(
            correlator.snapshot(), stream_positions(&source_offsets, &consumer_groups),
            window_outputs(windows.iter().filter(|window| !existing_window_keys.contains(&window_key(window))).cloned()),
            backend.as_ref(),
        ).await?;

        let mut final_consumer_lag = BTreeMap::new();
        for source in SignalSource::ALL {
            let lag = backend
                .consumer_lag(
                    &topics[&source].kafka_topic_name(),
                    &consumer_groups[&source],
                )
                .await?
                .into_iter()
                .map(|partition| partition.lag)
                .sum::<i64>();
            final_consumer_lag.insert(source.as_str().to_owned(), lag);
        }
        if final_consumer_lag.values().any(|lag| *lag != 0) {
            return Err(error(format!(
                "Kafka consumer lag remained after final checkpoint: {final_consumer_lag:?}"
            )));
        }

        Ok(StreamReplay {
            windows,
            stats,
            recovery: RecoveryReport {
                checkpoint_writes: 2,
                restored_generation,
                pre_crash_records,
                replayed_records_deduplicated: recovery_redeliveries,
                committed_offsets: source_positions(committed.positions())?.into_iter()
                    .map(|(source, position)| (source.as_str().to_owned(), position)).collect(),
                final_consumer_lag,
            },
            checkpoints,
        })
    }
    .await;

    for topic in topics.values() {
        let _ = backend.delete_topic(&topic.kafka_topic_name()).await;
    }
    replay
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), AnyError> {
    let root = example_root();
    let write_results = std::env::args().any(|argument| argument == "--write-results");
    let governance = VerifiedModelLock::load(root.join("model.lock.json"))?;
    governance.verify_model(MODEL)?;
    let questions = governance
        .load_contracts(&root)?
        .into_iter()
        .map(|(name, bytes)| Ok((name, serde_json::from_slice(&bytes)?)))
        .collect::<Result<BTreeMap<String, Value>, serde_json::Error>>()?;
    let fixture_names = [
        "baseline.json",
        "log-noise.json",
        "pool-exhaustion.json",
        "ambiguous-regression.json",
    ];
    let fixtures = fixture_names
        .iter()
        .map(|name| load_fixture(&root.join("fixtures").join(name)))
        .collect::<Result<Vec<_>, _>>()?;
    let redis = RedisConfig {
        url: std::env::var("ACTEON_CHECKPOINT_REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:16379".into()),
        prefix: format!("neural-simulation:{}", uuid::Uuid::new_v4()),
        ..RedisConfig::default()
    };
    let mut stream_replay = kafka_stream_replay(&fixtures, &redis).await?;
    let rules = fs::read_to_string(root.join("rules/verdict-routing.yaml"))?;
    let acteon = Arc::new(ActeonSimulation::build_with_store(
        &rules,
        Arc::new(RedisStateStore::new(&redis)?),
    )?);
    let laya = LayaClient::new(&root, governance.clone())?;
    let health = laya.health().await?;
    governance.verify_served_identity(&health.loaded, &health.revisions)?;
    let revision = governance.policy.model.revision.clone();
    let mut reports = Vec::new();
    let mut latencies = Vec::new();
    let verdict_key = stream_checkpoint_key(NAMESPACE, TENANT, "verdicts");
    let mut verdicts = StreamCheckpointCoordinator::<Vec<TrialReport>, Action>::initialize(
        Arc::new(RedisStateStore::new(&redis)?),
        verdict_key.clone(),
        Vec::new(),
        StreamCheckpointConfig::default(),
    )
    .await?;

    println!("\nACTEON + LAYA NEURAL OBSERVABILITY SIMULATION");
    println!("checkpoint: {MODEL} @ {revision} ({})\n", health.device);
    println!("governance: {} verified\n", governance.lock_digest);
    println!(
        "kafka: {} accepted records, {} duplicates/redeliveries, {} event-time windows",
        stream_replay.stats.accepted_records,
        stream_replay.stats.duplicate_records
            + stream_replay.recovery.replayed_records_deduplicated,
        stream_replay.windows.len()
    );
    println!(
        "recovery: restored generation {}, deduplicated {} replayed records, final lag {}\n",
        stream_replay.recovery.restored_generation,
        stream_replay.recovery.replayed_records_deduplicated,
        stream_replay
            .recovery
            .final_consumer_lag
            .values()
            .sum::<i64>()
    );

    for window in &stream_replay.windows {
        if !window.has_all_source_records() {
            return Err(error(format!(
                "window {} closed without all source records",
                window.window_id
            )));
        }
        let missing_sources = window.missing_sources();
        if !missing_sources.is_empty() {
            println!(
                "{} availability mask: {:?}",
                window.window_id, missing_sources
            );
        }
        let fixture = fixtures
            .iter()
            .find(|fixture| fixture.window_id == window.window_id)
            .ok_or_else(|| error(format!("no fixture metadata for {}", window.window_id)))?;
        let metrics_record = window
            .signal(SignalSource::Metrics)
            .ok_or_else(|| error("correlated window omitted metrics"))?;
        let traces_record = window
            .signal(SignalSource::Traces)
            .ok_or_else(|| error("correlated window omitted traces"))?;
        let logs_record = window
            .signal(SignalSource::Logs)
            .ok_or_else(|| error("correlated window omitted logs"))?;
        for (source, actual, expected) in [
            ("metrics", &metrics_record.features, &fixture.metrics),
            ("traces", &traces_record.features, &fixture.traces),
            ("logs", &logs_record.features, &fixture.logs),
        ] {
            if actual != expected {
                return Err(error(format!(
                    "Kafka window {} attached the wrong {source} features",
                    window.window_id
                )));
            }
        }
        let (metrics_call, traces_call, logs_call) = tokio::try_join!(
            laya.evaluate(
                "metrics",
                canonical_model_state(SignalSource::Metrics, &metrics_record.features)?,
                &questions["metrics"]
            ),
            laya.evaluate(
                "traces",
                canonical_model_state(SignalSource::Traces, &traces_record.features)?,
                &questions["traces"]
            ),
            laya.evaluate(
                "logs",
                canonical_model_state(SignalSource::Logs, &logs_record.features)?,
                &questions["logs"]
            ),
        )?;
        let signals = vec![
            signal_summary(
                "metrics",
                metrics_call,
                "pool_exhausted",
                metrics_record.available,
            )?,
            signal_summary(
                "traces",
                traces_call,
                "dependency_bottleneck",
                traces_record.available,
            )?,
            signal_summary(
                "logs",
                logs_call,
                "pool_timeout_present",
                logs_record.available,
            )?,
        ];
        let fusion_call = laya
            .evaluate(
                "fusion",
                fusion_state(fixture, &signals),
                &questions["fusion"],
            )
            .await?;
        let fusion = fusion_summary(fusion_call)?;
        for signal in &signals {
            latencies.push(signal.model_request_ms);
        }
        latencies.push(fusion.model_request_ms);
        let (route, evidence_strength, uncertainty) = choose_policy(&signals, &fusion);
        let verdict_action = outcome_action(
            fixture,
            route,
            evidence_strength,
            uncertainty,
            &fusion,
            &governance,
        );
        let acteon_outcome = "pending durable delivery".to_owned();
        let passed = route == fixture.expected_route;
        println!(
            "{:<22} signals=[{}, {}, {}] fusion={} ({:.2}) route={} -> {} [{}]",
            fixture.name,
            signals[0].condition,
            signals[1].condition,
            signals[2].condition,
            fusion.incident_kind,
            fusion.answer_confidence,
            route,
            acteon_outcome,
            if passed { "PASS" } else { "FAIL" }
        );
        reports.push(TrialReport {
            name: fixture.name.clone(),
            window_id: fixture.window_id.clone(),
            expected_route: fixture.expected_route.clone(),
            admitted_route: route.to_owned(),
            evidence_strength,
            uncertainty,
            source_positions: window
                .signals
                .iter()
                .map(|(source, record)| (source.as_str().to_owned(), record.position.clone()))
                .collect(),
            signals,
            fusion,
            acteon_outcome,
            passed,
        });
        verdicts
            .checkpoint(
                reports.clone(),
                [],
                [StreamOutboxEntry {
                    idempotency_key: fixture.window_id.clone(),
                    created_at: Utc::now(),
                    payload: verdict_action,
                }],
            )
            .await?;
        // The decision is durable before the corresponding input window is acknowledged.
        stream_replay
            .checkpoints
            .acknowledge_outputs([window_key(window)])
            .await?;
    }

    let calls_before_delivery = laya.calls.load(Ordering::SeqCst);
    let mut delivery = deliver_verdicts(
        &redis,
        verdict_key.clone(),
        Arc::clone(&acteon),
        &mut reports,
    )
    .await?;
    delivery.model_calls_repeated_on_retry =
        laya.calls.load(Ordering::SeqCst) - calls_before_delivery;
    delivery.pending_window_outputs = stream_replay.checkpoints.snapshot().pending_outputs().len();
    let duplicate_dispatches_prevented = delivery.deduplicated_redeliveries;
    if duplicate_dispatches_prevented != 1 || laya.calls.load(Ordering::SeqCst) != 16 {
        return Err(error(
            "delivery retry must deduplicate once and must not repeat inference",
        ));
    }
    verdicts.reload().await?;
    verdicts.checkpoint(reports.clone(), [], []).await?;
    for report in &reports {
        println!(
            "{} -> {} [{}]",
            report.name,
            report.acteon_outcome,
            if report.passed { "PASS" } else { "FAIL" }
        );
    }
    acteon.notification_intake.assert_called(0);
    if reports.iter().any(|trial| !trial.passed) {
        return Err(error("one or more trials failed their expected route"));
    }
    for (provider, actual) in [
        ("diagnostics", acteon.diagnostics.call_count()),
        ("on-call", acteon.on_call.call_count()),
        ("investigator", acteon.investigator.call_count()),
    ] {
        if actual != 1 {
            return Err(error(format!(
                "expected one {provider} call, observed {actual}"
            )));
        }
    }

    latencies.sort_by(f64::total_cmp);
    let report = SimulationReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        model: MODEL.to_owned(),
        health,
        governance,
        windowing: stream_replay.stats,
        recovery: stream_replay.recovery,
        aggregate: AggregateReport {
            model_calls: latencies.len(),
            total_model_request_ms: latencies.iter().sum(),
            p50_model_request_ms: percentile(&latencies, 50, 100),
            p95_model_request_ms: percentile(&latencies, 95, 100),
            diagnostic_calls: acteon.diagnostics.call_count(),
            on_call_notifications: acteon.on_call.call_count(),
            investigator_calls: acteon.investigator.call_count(),
            duplicate_dispatches_prevented,
        },
        trials: reports,
        delivery,
    };
    println!(
        "\n{} calls, {:.0} ms model HTTP elapsed, p50 {:.0} ms, p95 {:.0} ms",
        report.aggregate.model_calls,
        report.aggregate.total_model_request_ms,
        report.aggregate.p50_model_request_ms,
        report.aggregate.p95_model_request_ms
    );
    println!("all four trials passed; incident replay produced no second side effect\n");

    if write_results {
        let results = root.join("results");
        fs::create_dir_all(&results)?;
        fs::write(
            results.join("latest.json"),
            format!("{}\n", serde_json::to_string_pretty(&report)?),
        )?;
        fs::write(results.join("latest.md"), markdown(&report))?;
        println!("wrote {}", results.join("latest.md").display());
    }
    let store = RedisStateStore::new(&redis)?;
    store
        .delete(&stream_checkpoint_key(NAMESPACE, TENANT, "windows"))
        .await?;
    store.delete(&verdict_key).await?;
    laya.gateway.shutdown().await;
    acteon.routing_gateway.shutdown().await;
    acteon.gateway.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(source: &str, condition: &str, available: bool) -> SignalSummary {
        SignalSummary {
            source: source.to_owned(),
            condition: condition.to_owned(),
            condition_confidence: 0.5,
            evidence_probability: 0.5,
            impact_score: 2.0,
            available,
            model_request_ms: 1.0,
            wall_ms: 1.0,
            raw_response: json!({}),
            provider_evidence: BTreeMap::new(),
        }
    }

    fn fusion(kind: &str, confidence: f64) -> FusionSummary {
        FusionSummary {
            incident_kind: kind.to_owned(),
            answer_confidence: confidence,
            probabilities: BTreeMap::new(),
            correlated_probability: 0.5,
            impact_score: 2.0,
            model_request_ms: 1.0,
            wall_ms: 1.0,
            raw_response: json!({}),
            provider_evidence: BTreeMap::new(),
        }
    }

    #[test]
    fn policy_routes_measured_live_laya_decisions() {
        let cases = [
            (
                vec![
                    signal("metrics", "healthy", true),
                    signal("traces", "healthy", true),
                    signal("logs", "healthy", true),
                ],
                fusion("db_pool_exhaustion", 0.3185),
                "suppress",
            ),
            (
                vec![
                    signal("metrics", "healthy", true),
                    signal("traces", "healthy", true),
                    signal("logs", "healthy", true),
                ],
                fusion("db_pool_exhaustion", 0.3108),
                "suppress",
            ),
            (
                vec![
                    signal("metrics", "db_pool_pressure", true),
                    signal("traces", "database_wait", true),
                    signal("logs", "pool_timeout", true),
                ],
                fusion("db_pool_exhaustion", 0.3889),
                "incident",
            ),
            (
                vec![
                    signal("metrics", "application_errors", true),
                    signal("traces", "application_work", true),
                    signal("logs", "downstream_error", false),
                ],
                fusion("db_pool_exhaustion", 0.2919),
                "investigate",
            ),
        ];

        for (signals, fusion, expected) in cases {
            assert_eq!(choose_policy(&signals, &fusion).0, expected);
        }
    }

    #[test]
    fn cached_reports_survive_json_round_trip() -> Result<(), AnyError> {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../examples/neural-observability-detector/results/latest.json"
        ))?;
        let reports: Vec<TrialReport> = serde_json::from_value(fixture["trials"].clone())?;
        let encoded = serde_json::to_string(&reports)?;
        let decoded: Vec<TrialReport> = serde_json::from_str(&encoded)?;
        assert_eq!(
            serde_json::to_value(&reports)?,
            serde_json::to_value(&decoded)?
        );
        Ok(())
    }

    #[test]
    fn model_state_has_schema_order_independent_of_object_order() -> Result<(), AnyError> {
        let state = canonical_model_state(
            SignalSource::Metrics,
            &json!({
                "evidence_refs": ["metrics:0:1"],
                "db_pool_utilization": 0.96,
                "context": "Database pool saturation",
                "error_rate": 0.08
            }),
        )?;
        let fields = state
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| string_at(entry, &["field"]))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            fields,
            [
                "context",
                "error_rate",
                "db_pool_utilization",
                "evidence_refs"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn acteon_executes_the_three_policy_paths_once() -> Result<(), AnyError> {
        let root = example_root();
        let rules = fs::read_to_string(root.join("rules/verdict-routing.yaml"))?;
        let acteon = Arc::new(ActeonSimulation::build(&rules)?);

        for (key, band, expected) in [
            ("healthy", "suppress", "suppressed"),
            ("noise", "suppress", "suppressed"),
            ("pool", "incident", "completed"),
            ("ambiguous", "investigate", "rerouted"),
        ] {
            let outcome = acteon
                .dispatch(
                    Action::new(
                        NAMESPACE,
                        TENANT,
                        "verdict-audit",
                        "detector.verdict",
                        json!({
                            "incident_key": key,
                            "policy_band": band,
                            "evidence_refs": []
                        }),
                    )
                    .with_dedup_key(key),
                )
                .await?;
            assert!(
                outcome.starts_with(expected),
                "unexpected outcome: {outcome}"
            );
        }

        let duplicate = acteon
            .dispatch(
                Action::new(
                    NAMESPACE,
                    TENANT,
                    "verdict-audit",
                    "detector.verdict",
                    json!({"incident_key": "pool", "policy_band": "incident", "evidence_refs": []}),
                )
                .with_dedup_key("pool"),
            )
            .await?;
        assert_eq!(duplicate, "deduplicated by Acteon");
        acteon.notification_intake.assert_called(0);
        acteon.diagnostics.assert_called(1);
        acteon.on_call.assert_called(1);
        acteon.investigator.assert_called(1);
        acteon.gateway.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn receiver_rejects_a_cross_tenant_verdict_before_gateway_dispatch()
    -> Result<(), AnyError> {
        let rules = fs::read_to_string(example_root().join("rules/verdict-routing.yaml"))?;
        let acteon = Arc::new(ActeonSimulation::build(&rules)?);
        let delivery = GatewayDelivery {
            acteon: Arc::clone(&acteon),
            outcomes: Arc::new(Mutex::new(BTreeMap::new())),
            lose_incident_ack: AtomicBool::new(false),
            deduplicated: Arc::new(AtomicUsize::new(0)),
        };
        let entry = StreamOutboxEntry {
            idempotency_key: "foreign".into(),
            created_at: Utc::now(),
            payload: Action::new(
                NAMESPACE,
                "other-tenant",
                "verdict-audit",
                "detector.verdict",
                json!({"schema_version": 1, "policy_band": "incident"}),
            )
            .with_dedup_key("foreign"),
        };
        assert!(matches!(
            delivery.deliver(&entry).await,
            Err(StreamDeliveryError::Permanent(_))
        ));
        acteon.diagnostics.assert_called(0);
        acteon.on_call.assert_called(0);
        acteon.investigator.assert_called(0);
        acteon.routing_gateway.shutdown().await;
        acteon.gateway.shutdown().await;
        Ok(())
    }

    #[test]
    fn checkpoint_recovery_is_independent_of_cross_topic_delivery_order() -> Result<(), AnyError> {
        let fixtures = [
            "baseline.json",
            "log-noise.json",
            "pool-exhaustion.json",
            "ambiguous-regression.json",
        ]
        .iter()
        .map(|name| load_fixture(&example_root().join("fixtures").join(name)))
        .collect::<Result<Vec<_>, _>>()?;
        let mut lanes = BTreeMap::new();
        for source in SignalSource::ALL {
            let mut records = Vec::new();
            for (index, fixture) in fixtures.iter().enumerate() {
                let event = telemetry_event(fixture, source);
                let mut message = BusMessage::new(source.as_str(), serde_json::to_value(&event)?)
                    .with_key(event.correlation_key());
                message.partition = Some(0);
                message.offset = Some(i64::try_from(records.len())?);
                records.push(message.clone());
                if source == SignalSource::Metrics && index == 0 {
                    message.offset = Some(i64::try_from(records.len())?);
                    records.push(message);
                }
            }
            lanes.insert(source, records);
        }
        let sources = SignalSource::ALL;
        let mut schedules = Vec::new();
        for a in 0..3 {
            for b in 0..3 {
                if a != b {
                    let c = 3 - a - b;
                    schedules.push(
                        [sources[a], sources[b], sources[c]]
                            .iter()
                            .flat_map(|source| lanes[source].clone())
                            .collect::<Vec<_>>(),
                    );
                }
            }
        }
        let mut round_robin = Vec::new();
        for index in 0..5 {
            for source in sources {
                if let Some(message) = lanes[&source].get(index) {
                    round_robin.push(message.clone());
                }
            }
        }
        schedules.push(round_robin);
        let groups = sources
            .map(|source| (source, source.as_str().to_owned()))
            .into_iter()
            .collect();
        for before in &schedules {
            for after in &schedules {
                let mut correlator = EventTimeCorrelator::new(
                    ChronoDuration::minutes(1),
                    ChronoDuration::seconds(15),
                )?;
                let mut offsets = BTreeMap::new();
                let mut windows = Vec::new();
                for message in before.iter().take(5) {
                    remember_source_position(message, &mut offsets)?;
                    windows.extend(correlator.ingest(message.clone())?.emitted);
                }
                let saved_positions = source_positions(&stream_positions(&offsets, &groups))?;
                let mut restored = EventTimeCorrelator::restore(correlator.snapshot())?;
                let mut skipped = 0;
                for message in after {
                    let (source, position) = remember_source_position(message, &mut offsets)?;
                    if is_recovery_record(source, &position, &saved_positions) {
                        skipped += 1;
                        continue;
                    }
                    let result = restored.ingest(message.clone())?;
                    assert_ne!(result.disposition, IngestDisposition::Late);
                    windows.extend(result.emitted);
                }
                windows.extend(restored.finish()?);
                assert_eq!(skipped, 5);
                assert_eq!(windows.len(), 4);
                assert_eq!(restored.stats().accepted_records, 12);
                assert_eq!(restored.stats().duplicate_records, 1);
                assert_eq!(restored.stats().late_records, 0);
            }
        }
        Ok(())
    }
}
