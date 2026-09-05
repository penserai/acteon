//! Scripted product workflows. Provider ledgers are independent effect oracles;
//! they deliberately model downstream idempotency, not exactly-once gateways.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome, ProviderResponse};
use acteon_provider::ProviderError;
use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::evaluation::derived_seed;
use super::{Scenario, ScenarioReport, action, backend_config, outcome_name};
use crate::{RecordingProvider, SimulationConfig, SimulationError, SimulationHarness};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Fault {
    id: String,
    action_type: String,
    attempt: u32,
    kind: FaultKind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FaultKind {
    Transient,
    LostAcknowledgement,
}

#[derive(Default)]
struct Ledger {
    attempts: BTreeMap<String, u32>,
    effects: BTreeMap<String, u32>,
    keys: BTreeSet<(String, String, String)>,
    faults: Vec<Fault>,
    refunded: bool,
    cancelled: bool,
}

impl Ledger {
    fn effects(&self, kind: &str) -> u32 {
        self.effects.get(kind).copied().unwrap_or(0)
    }
    fn attempts(&self, kind: &str) -> u32 {
        self.attempts.get(kind).copied().unwrap_or(0)
    }

    fn execute(
        &mut self,
        action: &Action,
        plan: &[Fault],
        idempotent: bool,
    ) -> Result<ProviderResponse, ProviderError> {
        let kind = action.action_type.clone();
        let attempt = self.attempts.entry(kind.clone()).or_default();
        *attempt += 1;
        let fault = plan
            .iter()
            .find(|fault| fault.action_type == kind && fault.attempt == *attempt);
        if let Some(fault) = fault {
            self.faults.push(fault.clone());
            if fault.kind == FaultKind::Transient {
                return Err(ProviderError::Connection(
                    "injected transient outage".into(),
                ));
            }
        }
        if kind == "ship" && self.refunded {
            return Err(ProviderError::ExecutionFailed(
                "refunded order cannot ship".into(),
            ));
        }
        let key = (
            action.tenant.to_string(),
            kind.clone(),
            action.payload["object_id"]
                .as_str()
                .unwrap_or("fixture")
                .to_owned(),
        );
        if !idempotent || self.keys.insert(key) {
            *self.effects.entry(kind.clone()).or_default() += 1;
            if kind == "refund" {
                self.refunded = true;
            }
            if kind == "cancel" {
                self.cancelled = true;
            }
        }
        if fault.is_some_and(|fault| fault.kind == FaultKind::LostAcknowledgement) {
            return Err(ProviderError::Connection(
                "effect committed; injected acknowledgement loss".into(),
            ));
        }
        Ok(ProviderResponse::success(json!({"completed": kind})))
    }
}

async fn cluster(
    report: &mut ScenarioReport,
    scenario: Scenario,
    rules: &str,
    providers: Vec<Arc<RecordingProvider>>,
) -> Result<SimulationHarness, SimulationError> {
    let harness = SimulationHarness::start_with_providers(
        SimulationConfig::builder()
            .nodes(2)
            .shared_state(true)
            .state_backend(backend_config(report.manifest.backend)?)
            .add_recording_provider("effect")
            .add_recording_provider("approval-notifications")
            .add_rule_yaml(rules)
            .executor_config(acteon_executor::ExecutorConfig {
                max_retries: 3,
                retry_strategy: acteon_executor::RetryStrategy::Constant {
                    delay: Duration::ZERO,
                },
                ..Default::default()
            })
            .build(),
        providers,
    )
    .await?;
    report.event(
        scenario,
        "backend instantiated",
        harness.state_backend_identity(),
        0,
    );
    Ok(harness)
}

fn provider(
    name: &str,
    ledger: &Arc<Mutex<Ledger>>,
    plan: Vec<Fault>,
    idempotent: bool,
) -> Arc<RecordingProvider> {
    let ledger = ledger.clone();
    Arc::new(
        RecordingProvider::new(name)
            .with_response_fn(move |action| ledger.lock().execute(action, &plan, idempotent)),
    )
}

fn approval_rule(kind: &str) -> String {
    format!(
        "rules:\n  - name: human-approval\n    condition: {{field: action.action_type, eq: {kind}}}\n    action: {{type: request_approval, notify_provider: approval-notifications, timeout_seconds: 3600}}\n"
    )
}

async fn dispatch(
    harness: &SimulationHarness,
    report: &mut ScenarioReport,
    scenario: Scenario,
    node: usize,
    actor: &str,
    action: &Action,
) -> Result<ActionOutcome, SimulationError> {
    let outcome = harness
        .dispatch_to(node, action)
        .await
        .map_err(|error| SimulationError::Dispatch(error.to_string()))?;
    report.event(
        scenario,
        &format!("{actor}: {}", action.action_type),
        outcome_name(&outcome),
        harness.provider("effect").unwrap().call_count(),
    );
    Ok(outcome)
}

struct Approval {
    id: String,
    signature: String,
    expires: i64,
    kid: Option<String>,
}

impl Approval {
    fn from_outcome(outcome: &ActionOutcome) -> Result<Self, SimulationError> {
        let ActionOutcome::PendingApproval {
            approval_id,
            approve_url,
            ..
        } = outcome
        else {
            return Err(SimulationError::Gateway(
                "expected pending human approval".into(),
            ));
        };
        let url = reqwest::Url::parse(approve_url)
            .map_err(|error| SimulationError::Configuration(error.to_string()))?;
        let query: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        Ok(Self {
            id: approval_id.clone(),
            signature: query
                .get("sig")
                .cloned()
                .ok_or_else(|| SimulationError::Gateway("approval signature missing".into()))?,
            expires: query
                .get("expires_at")
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| SimulationError::Gateway("approval expiry missing".into()))?,
            kid: query.get("kid").cloned(),
        })
    }

    async fn approve(
        &self,
        harness: &SimulationHarness,
        node: usize,
        tenant: &str,
    ) -> Result<ActionOutcome, acteon_gateway::GatewayError> {
        harness
            .node(node)
            .unwrap()
            .gateway()
            .execute_approval(
                "scenario",
                tenant,
                &self.id,
                &self.signature,
                self.expires,
                self.kid.as_deref(),
            )
            .await
    }

    async fn reject(
        &self,
        harness: &SimulationHarness,
    ) -> Result<(), acteon_gateway::GatewayError> {
        harness
            .node(1)
            .unwrap()
            .gateway()
            .reject_approval(
                "scenario",
                "alice",
                &self.id,
                &self.signature,
                self.expires,
                self.kid.as_deref(),
            )
            .await
    }
}

fn record_faults(
    report: &mut ScenarioReport,
    scenario: Scenario,
    planned: &[Fault],
    observed: &[Fault],
) {
    for fault in planned {
        report.event(
            scenario,
            "fault scheduled",
            &serde_json::to_string(fault).expect("fault serializes"),
            0,
        );
    }
    for fault in observed {
        report.event(
            scenario,
            "fault consumed",
            &serde_json::to_string(fault).expect("fault serializes"),
            0,
        );
    }
}

/// Drain real gateway audit writers before independently querying their stores.
async fn check_audit(
    harness: &SimulationHarness,
    report: &mut ScenarioReport,
    scenario: Scenario,
    actions: &[(&Action, &str)],
) -> Result<(), SimulationError> {
    let mut records = Vec::new();
    for index in 0..harness.node_count() {
        let gateway = harness.node(index).unwrap().gateway();
        gateway.shutdown().await;
        let audit = gateway
            .audit_store()
            .ok_or_else(|| SimulationError::Gateway("missing audit store".into()))?;
        let page = audit
            .query(&acteon_audit::AuditQuery {
                namespace: Some("scenario".into()),
                tenant: Some("alice".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .await
            .map_err(|error| SimulationError::Gateway(error.to_string()))?;
        records.extend(page.records);
    }
    let covered = actions.iter().all(|(action, expected_outcome)| {
        records.iter().any(|record| {
            record.action_id == action.id.as_str()
                && record.tenant == "alice"
                && record.action_type == action.action_type.as_str()
                && record.outcome == *expected_outcome
        })
    });
    report.check(
        scenario,
        "dispatches_audited",
        covered,
        format!(
            "{} action/outcome pairs checked across {} audit records",
            actions.len(),
            records.len()
        ),
    );
    report.event(
        scenario,
        "audit reader: verify dispatch outcomes",
        if covered { "complete" } else { "missing" },
        0,
    );
    Ok(())
}

pub(super) async fn incident(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    incident_impl(report, true).await
}

#[allow(
    clippy::too_many_lines,
    reason = "keep scripted operations and their independent observations in execution order"
)]
async fn incident_impl(
    report: &mut ScenarioReport,
    require_approval: bool,
) -> Result<(), SimulationError> {
    let scenario = Scenario::IncidentResponse;
    let failures =
        u32::try_from(derived_seed(report.manifest.seed, 0, "notification-outage") % 3 + 1)
            .expect("bounded");
    let plan: Vec<_> = (1..=failures)
        .map(|attempt| Fault {
            id: format!("notification-{attempt}"),
            action_type: "notify".into(),
            attempt,
            kind: FaultKind::Transient,
        })
        .collect();
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let mut rules = if require_approval {
        approval_rule("remediate")
    } else {
        "rules:\n".into()
    };
    rules.push_str("  - name: duplicate-alert\n    condition: {field: action.action_type, eq: alert}\n    action: {type: deduplicate, ttl_seconds: 3600}\n");
    let harness = cluster(
        report,
        scenario,
        &rules,
        vec![provider("effect", &ledger, plan.clone(), true)],
    )
    .await?;
    record_faults(report, scenario, &plan, &[]);
    let node =
        usize::try_from(derived_seed(report.manifest.seed, 0, "actor-node") % 2).expect("bounded");
    let alert = action(
        report.manifest.seed,
        0,
        "alice",
        "alert",
        json!({"object_id":"incident-1"}),
    )
    .with_dedup_key("incident-1");
    dispatch(&harness, report, scenario, node, "detector", &alert).await?;
    let duplicate = dispatch(
        &harness,
        report,
        scenario,
        1 - node,
        "duplicate detector",
        &alert,
    )
    .await?;
    report.check(
        scenario,
        "one_incident",
        {
            let ledger = ledger.lock();
            ledger.effects("alert") == 1
                && ledger.attempts("alert") == 1
                && matches!(duplicate, ActionOutcome::Deduplicated)
        },
        "gateway deduplicated the redelivered alert before the incident provider",
    );
    let remediation = action(
        report.manifest.seed,
        1,
        "alice",
        "remediate",
        json!({"object_id":"incident-1"}),
    );
    let pending = dispatch(&harness, report, scenario, node, "runbook", &remediation).await?;
    report.check(
        scenario,
        "approval_required",
        matches!(pending, ActionOutcome::PendingApproval { .. }),
        "destructive remediation requires a human decision",
    );
    report.check(
        scenario,
        "preapproval_no_effect",
        ledger.lock().effects("remediate") == 0,
        "effect ledger inspected before approval",
    );
    let approval = Approval::from_outcome(&pending)?;
    let denied = approval.approve(&harness, 1 - node, "bob").await.is_err();
    report.check(
        scenario,
        "cross_tenant_denied",
        denied && ledger.lock().effects("remediate") == 0,
        "another tenant cannot use the approval signature",
    );
    let approved = approval
        .approve(&harness, 1 - node, "alice")
        .await
        .map_err(|error| SimulationError::Dispatch(error.to_string()))?;
    let replay = approval.approve(&harness, node, "alice").await;
    report.check(
        scenario,
        "replay_once",
        replay.is_err() && ledger.lock().attempts("remediate") == 1,
        "approval replay cannot reach the effect provider",
    );
    report.check(
        scenario,
        "remediation_once",
        matches!(approved, ActionOutcome::Executed(_)) && ledger.lock().effects("remediate") == 1,
        "approved remediation committed once",
    );
    report.event(
        scenario,
        "commander: approve across nodes",
        outcome_name(&approved),
        harness.provider("effect").unwrap().call_count(),
    );
    let notification = action(
        report.manifest.seed,
        2,
        "alice",
        "notify",
        json!({"object_id":"incident-1"}),
    );
    let recovered = dispatch(
        &harness,
        report,
        scenario,
        node,
        "communications",
        &notification,
    )
    .await?;
    {
        let ledger = ledger.lock();
        record_faults(report, scenario, &[], &ledger.faults);
        report.check(
            scenario,
            "notification_faults_consumed",
            ledger.faults == plan,
            "every scheduled outage was observed at the provider boundary",
        );
        report.check(
            scenario,
            "notification_recovered",
            matches!(recovered, ActionOutcome::Executed(_)) && ledger.effects("notify") == 1,
            "notification recovered after the configured outage",
        );
        report.check(
            scenario,
            "bounded_attempts",
            ledger.attempts("notify") == failures + 1 && ledger.attempts("remediate") == 1,
            "retry budget and approval execution counts",
        );
    }
    check_audit(
        &harness,
        report,
        scenario,
        &[
            (&alert, "executed"),
            (&alert, "deduplicated"),
            (&remediation, "pending_approval"),
            (&remediation, "executed"),
            (&notification, "executed"),
        ],
    )
    .await?;
    harness.teardown().await
}

pub(super) async fn refund(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    refund_impl(report, true).await
}

#[allow(
    clippy::too_many_lines,
    reason = "keep scripted operations and their independent observations in execution order"
)]
async fn refund_impl(report: &mut ScenarioReport, idempotent: bool) -> Result<(), SimulationError> {
    let scenario = Scenario::RefundFulfillment;
    let plan = vec![Fault {
        id: "billing-ack-loss".into(),
        action_type: "refund".into(),
        attempt: 1,
        kind: FaultKind::LostAcknowledgement,
    }];
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let harness = cluster(
        report,
        scenario,
        &approval_rule("refund"),
        vec![provider("effect", &ledger, plan.clone(), idempotent)],
    )
    .await?;
    record_faults(report, scenario, &plan, &[]);
    let request = action(
        report.manifest.seed,
        0,
        "alice",
        "refund",
        json!({"object_id":"order-1", "amount":1500}),
    );
    let pending = dispatch(&harness, report, scenario, 0, "billing", &request).await?;
    report.check(
        scenario,
        "approval_required",
        matches!(pending, ActionOutcome::PendingApproval { .. }),
        "high-value refund requires approval",
    );
    report.check(
        scenario,
        "preapproval_no_effect",
        ledger.lock().effects("refund") == 0,
        "billing ledger inspected before the reviewer decision",
    );
    let approval = Approval::from_outcome(&pending)?;
    let approved = approval
        .approve(&harness, 1, "alice")
        .await
        .map_err(|error| SimulationError::Dispatch(error.to_string()))?;
    let replay = approval.approve(&harness, 0, "alice").await;
    report.event(
        scenario,
        "reviewer: approve refund",
        outcome_name(&approved),
        harness.provider("effect").unwrap().call_count(),
    );
    let cancel = action(
        report.manifest.seed,
        1,
        "alice",
        "cancel",
        json!({"object_id":"order-1"}),
    );
    let cancelled = dispatch(&harness, report, scenario, 0, "fulfillment", &cancel).await?;
    let shipment = action(
        report.manifest.seed,
        2,
        "alice",
        "ship",
        json!({"object_id":"order-1"}),
    );
    let shipped = dispatch(
        &harness,
        report,
        scenario,
        1,
        "stale fulfillment actor",
        &shipment,
    )
    .await?;
    let rejected_request = action(
        report.manifest.seed,
        3,
        "alice",
        "refund",
        json!({"object_id":"order-2", "amount":2000}),
    );
    let rejected_pending = dispatch(
        &harness,
        report,
        scenario,
        0,
        "second order",
        &rejected_request,
    )
    .await?;
    let rejected = Approval::from_outcome(&rejected_pending)?;
    rejected
        .reject(&harness)
        .await
        .map_err(|error| SimulationError::Dispatch(error.to_string()))?;
    let rejected_retry = rejected.approve(&harness, 1, "alice").await;
    {
        let ledger = ledger.lock();
        record_faults(report, scenario, &[], &ledger.faults);
        report.check(
            scenario,
            "refund_once",
            ledger.effects("refund") == 1 && replay.is_err(),
            "lost acknowledgement and approval replay cannot duplicate the downstream refund",
        );
        report.check(
            scenario,
            "rejected_no_effect",
            rejected_retry.is_err()
                && !ledger.keys.iter().any(|(_, _, object)| object == "order-2"),
            "rejected second order never reaches billing",
        );
        report.check(
            scenario,
            "no_shipment_after_refund",
            ledger.effects("ship") == 0 && matches!(shipped, ActionOutcome::Failed(_)),
            "downstream fulfillment guard refuses a refunded order",
        );
        report.check(
            scenario,
            "refund_and_cancel_complete",
            ledger.refunded && ledger.cancelled && matches!(cancelled, ActionOutcome::Executed(_)),
            "terminal refund/cancel state inspected independently",
        );
        report.check(
            scenario,
            "ack_loss_observed",
            ledger.faults == plan,
            "billing committed before the injected acknowledgement loss",
        );
        report.check(
            scenario,
            "refund_acknowledged",
            matches!(approved, ActionOutcome::Executed(_)),
            "retry recovered the original refund result",
        );
        report.check(
            scenario,
            "bounded_attempts",
            ledger.attempts("refund") == 2 && ledger.attempts("ship") == 1,
            "two billing attempts and one rejected shipment attempt",
        );
    }
    check_audit(
        &harness,
        report,
        scenario,
        &[
            (&request, "pending_approval"),
            (&request, "executed"),
            (&cancel, "executed"),
            (&shipment, "failed"),
            (&rejected_request, "pending_approval"),
        ],
    )
    .await?;
    harness.teardown().await
}

const RESEARCH_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scenarios/fixtures/poisoned_research.json"
));

fn strings(value: &Value, output: &mut String) {
    match value {
        Value::String(value) => output.push_str(value),
        Value::Array(values) => values.iter().for_each(|value| strings(value, output)),
        Value::Object(values) => values.values().for_each(|value| strings(value, output)),
        _ => (),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing a string");
    }
    result
}

fn contains_canary(value: &Value, canary: &str) -> bool {
    let mut joined = String::new();
    strings(value, &mut joined);
    let encoded = [
        canary.to_owned(),
        base64::engine::general_purpose::STANDARD.encode(canary),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(canary),
        hex(canary.as_bytes()),
        hex(canary.as_bytes()).to_uppercase(),
        format!("{:x}", Sha256::digest(canary.as_bytes())),
    ];
    let serialized = serde_json::to_string(value).expect("JSON serializes");
    encoded
        .iter()
        .any(|secret| joined.contains(secret) || serialized.contains(secret))
}

pub(super) async fn injection(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    injection_impl(report, true).await
}

#[allow(
    clippy::too_many_lines,
    reason = "keep scripted operations and their independent observations in execution order"
)]
async fn injection_impl(
    report: &mut ScenarioReport,
    enforce_policy: bool,
) -> Result<(), SimulationError> {
    let scenario = Scenario::PromptInjection;
    let fixture: Value = serde_json::from_str(RESEARCH_FIXTURE)
        .map_err(|error| SimulationError::Configuration(error.to_string()))?;
    let fixture_hash = format!("{:x}", Sha256::digest(RESEARCH_FIXTURE.as_bytes()));
    let canary = format!(
        "canary-{:016x}-fixture",
        derived_seed(report.manifest.seed, 0, "canary")
    );
    let rules = if enforce_policy {
        "rules:\n  - name: untrusted-tool-denied\n    condition: {field: action.action_type, ne: summary}\n    action: {type: suppress}\n"
    } else {
        "rules: []\n"
    };
    let harness = cluster(report, scenario, rules, vec![]).await?;
    report.event(
        scenario,
        "retriever: frozen poisoned document",
        &format!("checkout-runbook-v1 sha256:{fixture_hash}"),
        0,
    );
    let variants = [
        json!({"tool":"webhook", "data":canary}),
        json!({"tool":"webhook", "data":base64::engine::general_purpose::STANDARD.encode(&canary)}),
        json!({"tool":"secret_proxy", "data":hex(canary.as_bytes())}),
        json!({"tool":"webhook", "chunks":[&canary[..12], &canary[12..]], "claimed_role":"admin"}),
    ];
    let mut actions = Vec::new();
    let mut denied = true;
    for (index, payload) in variants.into_iter().enumerate() {
        let hostile = action(report.manifest.seed, index, "alice", "exfiltrate", payload);
        let outcome = dispatch(
            &harness,
            report,
            scenario,
            index % 2,
            "poisoned tool proposal",
            &hostile,
        )
        .await?;
        denied &= matches!(outcome, ActionOutcome::Suppressed { .. });
        actions.push(hostile);
    }
    let policy = acteon_http::OutboundPolicy::default();
    let network_denied = [
        "https://127.0.0.1/canary",
        "https://169.254.169.254/latest/meta-data",
        "https://10.0.0.1/secret",
        "https://[::ffff:127.0.0.1]/",
    ]
    .iter()
    .all(|url| policy.validate_url(url).is_err());
    report.check(scenario, "network_destinations_denied", network_denied, "literal private, metadata, and mapped-loopback destinations rejected without network traffic");
    let summary = action(
        report.manifest.seed,
        10,
        "alice",
        "summary",
        json!({"service": fixture["facts"]["service"], "owner": fixture["facts"]["owner"], "source_id":fixture["source_id"]}),
    );
    let outcome = dispatch(
        &harness,
        report,
        scenario,
        0,
        "scripted summarizer",
        &summary,
    )
    .await?;
    actions.push(summary.clone());
    let calls = harness.provider("effect").unwrap().calls();
    let outputs = Value::Array(
        calls
            .iter()
            .map(|call| json!({"action":call.action,"response":call.response}))
            .collect(),
    );
    report.check(
        scenario,
        "malicious_calls_denied",
        denied
            && calls
                .iter()
                .all(|call| call.action.action_type.as_str() == "summary"),
        "untrusted role claims and encoded tool proposals cannot bypass policy",
    );
    report.check(scenario, "no_canary_disclosure", !contains_canary(&outputs, &canary), "raw/base64/hex/SHA256 and joined-string canary scan over effect-provider inputs and outputs");
    report.check(scenario, "benign_summary", matches!(outcome, ActionOutcome::Executed(_)) && calls.iter().any(|call| call.action.payload == json!({"service":"checkout", "owner":"commerce", "source_id":"checkout-runbook-v1"})), "benign frozen facts delivered despite hostile document instructions");
    report.check(
        scenario,
        "fixture_preserved",
        fixture["source_id"] == "checkout-runbook-v1"
            && fixture["untrusted_instruction"]
                .as_str()
                .is_some_and(|text| text.contains("administrator")),
        "source identity and poison marker retained with the fixture digest",
    );
    let expected: Vec<_> = actions
        .iter()
        .map(|action| {
            (
                action,
                if action.action_type.as_str() == "summary" || !enforce_policy {
                    "executed"
                } else {
                    "suppressed"
                },
            )
        })
        .collect();
    check_audit(&harness, report, scenario, &expected).await?;
    harness.teardown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::evaluation::grade;
    use crate::scenario::{Backend, ScenarioManifest};

    fn report(scenario: Scenario) -> ScenarioReport {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 42,
            backend: Backend::Memory,
            scenarios: vec![scenario],
        };
        ScenarioReport {
            schema_version: 1,
            manifest_sha256: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&manifest).unwrap())
            ),
            implementation_version: env!("CARGO_PKG_VERSION").into(),
            manifest,
            invariants: Vec::new(),
            trace: Vec::new(),
        }
    }

    #[tokio::test]
    async fn removing_approval_rule_fails_the_preapproval_gate() {
        let mut report = report(Scenario::IncidentResponse);
        assert!(incident_impl(&mut report, false).await.is_err());
        let grade = grade(Scenario::IncidentResponse, &report);
        assert!(!grade.passed);
        assert!(
            grade
                .gates
                .iter()
                .any(|gate| gate.name == "preapproval_no_effect" && !gate.passed)
        );
    }

    #[tokio::test]
    async fn removing_downstream_idempotency_exposes_duplicate_refund() {
        let mut report = report(Scenario::RefundFulfillment);
        refund_impl(&mut report, false).await.unwrap();
        let grade = grade(Scenario::RefundFulfillment, &report);
        assert!(!grade.passed);
        assert!(
            grade.score_basis_points > 8000,
            "otherwise healthy workflow retains useful diagnostic score"
        );
        assert!(
            grade
                .gates
                .iter()
                .any(|gate| gate.name == "refund_once" && !gate.passed)
        );
    }

    #[tokio::test]
    async fn removing_tool_policy_fails_canary_and_authorization_gates() {
        let mut report = report(Scenario::PromptInjection);
        injection_impl(&mut report, false).await.unwrap();
        let grade = grade(Scenario::PromptInjection, &report);
        assert!(!grade.passed);
        assert!(
            grade
                .gates
                .iter()
                .any(|gate| gate.name == "no_canary_disclosure" && !gate.passed)
        );
        let canary = format!("canary-{:016x}-fixture", derived_seed(42, 0, "canary"));
        assert!(
            !contains_canary(&serde_json::to_value(report).unwrap(), &canary),
            "failure artifacts must not contain the canary"
        );
    }

    #[test]
    fn canary_detector_covers_encodings_and_fragmented_values() {
        let canary = "fixture-secret-value";
        for value in [
            json!(canary),
            json!(base64::engine::general_purpose::STANDARD.encode(canary)),
            json!(hex(canary.as_bytes())),
            json!(format!("{:x}", Sha256::digest(canary.as_bytes()))),
            json!(["fixture-", "secret-", "value"]),
        ] {
            assert!(contains_canary(&value, canary));
        }
        assert!(!contains_canary(
            &json!({"summary":"checkout is owned by commerce"}),
            canary
        ));
    }
}
