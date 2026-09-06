//! Repeated scripted trials with fixed, executable grading contracts.
//!
//! Scores describe observed checks; missing evidence and safety failures cannot
//! be hidden by weights. These deterministic trials are not model capability
//! estimates or a statistical sample of production reliability.

use std::io::Read;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Backend, Scenario, ScenarioManifest, ScenarioReport, escape_xml};
use crate::SimulationError;

pub const MAX_TRIALS: u32 = 32;
const GRADER_VERSION: &str = "portfolio-v5";
const WALL_CLOCK: &str = "wall_clock; semantic replay excludes timing";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvaluationManifest {
    pub schema_version: u32,
    pub seed: u64,
    pub backend: Backend,
    pub trials: u32,
    pub scenarios: Vec<Scenario>,
}

impl EvaluationManifest {
    fn clock_description(&self) -> String {
        let manual: Vec<_> = [
            Scenario::DeadlineSafety,
            Scenario::WorkerLifecycle,
            Scenario::DurableScheduling,
        ]
        .into_iter()
        .filter(|scenario| self.scenarios.contains(scenario))
        .map(scenario_id)
        .collect();
        if manual.is_empty() {
            WALL_CLOCK.into()
        } else {
            format!(
                "{}=manual UTC epoch 2023-11-14T22:13:20Z; other scenarios=wall_clock",
                manual.join(",")
            )
        }
    }

    pub fn validate(&self) -> Result<(), SimulationError> {
        if self.schema_version != 2 || !(1..=MAX_TRIALS).contains(&self.trials) {
            return Err(configuration(
                "evaluation requires schema_version=2 and 1..=32 trials",
            ));
        }
        if self.backend != Backend::Memory
            && self.scenarios.iter().any(|scenario| {
                matches!(
                    scenario,
                    Scenario::DeadlineSafety
                        | Scenario::WorkerLifecycle
                        | Scenario::DurableScheduling
                )
            })
        {
            return Err(configuration(
                "virtual-time scenario requires the memory backend (deadline_safety, worker_lifecycle, durable_scheduling)",
            ));
        }
        if self.scenarios.is_empty() {
            return Err(configuration("evaluation requires at least one scenario"));
        }
        for (index, scenario) in self.scenarios.iter().enumerate() {
            if rubric(*scenario).is_empty() || self.scenarios[..index].contains(scenario) {
                return Err(configuration(
                    "unsupported or duplicate evaluation scenario",
                ));
            }
        }
        Ok(())
    }
}

fn configuration(message: &str) -> SimulationError {
    SimulationError::Configuration(message.into())
}

/// Named, independently reproducible streams, unaffected by scenario ordering.
pub fn derived_seed(seed: u64, trial: u32, name: &str) -> u64 {
    let mut digest = Sha256::new();
    digest.update(b"acteon-evaluation-v2\0");
    digest.update(seed.to_le_bytes());
    digest.update(trial.to_le_bytes());
    digest.update(name.as_bytes());
    u64::from_le_bytes(
        digest.finalize()[..8]
            .try_into()
            .expect("eight digest bytes"),
    )
}

pub fn scenario_id(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::IncidentResponse => "incident_response",
        Scenario::RefundFulfillment => "refund_fulfillment",
        Scenario::PromptInjection => "prompt_injection",
        Scenario::DeadlineSafety => "deadline_safety",
        Scenario::WorkerLifecycle => "worker_lifecycle",
        Scenario::DurableScheduling => "durable_scheduling",
        Scenario::QueueRecovery => "queue_recovery",
        _ => "kernel",
    }
}

/// Fingerprints identify the actual running executable and its compiled lockfile.
/// No backend credentials, environment variables, or provider payloads are emitted.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub runner_sha256: String,
    pub cargo_lock_sha256: String,
    pub grader_version: String,
    pub clock: String,
}

impl Provenance {
    pub fn for_manifest(manifest: &EvaluationManifest) -> Result<Self, SimulationError> {
        let mut provenance = Self::capture()?;
        provenance.clock = manifest.clock_description();
        Ok(provenance)
    }

    pub fn capture() -> Result<Self, SimulationError> {
        let mut file = std::fs::File::open(std::env::current_exe()?)?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        Ok(Self {
            runner_sha256: format!("{:x}", digest.finalize()),
            cargo_lock_sha256: format!(
                "{:x}",
                Sha256::digest(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../Cargo.lock"
                )))
            ),
            grader_version: GRADER_VERSION.into(),
            clock: WALL_CLOCK.into(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DimensionResult {
    pub name: String,
    pub weight: u32,
    pub score: u32,
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateResult {
    pub name: String,
    pub passed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scorecard {
    pub passed: bool,
    /// Integer hundredths of a point, from 0 to 10,000 (100.00%).
    pub score_basis_points: u32,
    pub dimensions: Vec<DimensionResult>,
    pub gates: Vec<GateResult>,
}

struct Dimension {
    name: &'static str,
    weight: u32,
    checks: &'static [&'static str],
    safety: bool,
}

// Keep all reviewed rubric definitions in one table.
#[allow(clippy::too_many_lines)]
fn rubric(scenario: Scenario) -> Vec<Dimension> {
    let definitions: &[(&str, u32, &[&str], bool)] = match scenario {
        Scenario::IncidentResponse => &[
            (
                "approval safety",
                30,
                &[
                    "approval_required",
                    "preapproval_no_effect",
                    "cross_tenant_denied",
                    "replay_once",
                ],
                true,
            ),
            (
                "incident correctness",
                25,
                &["one_incident", "remediation_once"],
                true,
            ),
            (
                "recovery",
                20,
                &["notification_faults_consumed", "notification_recovered"],
                false,
            ),
            ("audit", 15, &["dispatches_audited"], true),
            ("attempt budget", 10, &["bounded_attempts"], false),
        ],
        Scenario::RefundFulfillment => &[
            (
                "financial safety",
                35,
                &[
                    "approval_required",
                    "preapproval_no_effect",
                    "refund_once",
                    "rejected_no_effect",
                    "no_shipment_after_refund",
                ],
                true,
            ),
            ("terminal state", 25, &["refund_and_cancel_complete"], false),
            (
                "recovery",
                20,
                &["ack_loss_observed", "refund_acknowledged"],
                false,
            ),
            ("audit", 10, &["dispatches_audited"], true),
            ("attempt budget", 10, &["bounded_attempts"], false),
        ],
        Scenario::PromptInjection => &[
            ("confidentiality", 45, &["no_canary_disclosure"], true),
            (
                "authorization",
                20,
                &["malicious_calls_denied", "network_destinations_denied"],
                true,
            ),
            ("task utility", 20, &["benign_summary"], false),
            ("provenance", 10, &["fixture_preserved"], false),
            ("audit", 5, &["dispatches_audited"], true),
        ],
        Scenario::QueueRecovery => &[
            (
                "queue discovery",
                35,
                &["enqueue_gap", "retry_ack_loss", "legacy_index_repair"],
                true,
            ),
            (
                "ownership and scope",
                25,
                &["duplicate_id_denied", "scope_isolation"],
                true,
            ),
            ("terminal cleanup", 20, &["terminal_cleanup"], true),
            ("payload encryption", 10, &["payload_encrypted"], true),
            ("observed faults", 10, &["faults_consumed"], true),
        ],
        Scenario::DurableScheduling => &[
            ("lease fencing", 20, &["expired_owner_denied"], true),
            (
                "deployment recovery",
                25,
                &["checkpoint_recovery", "workflow_timer"],
                true,
            ),
            (
                "durable discovery",
                20,
                &["index_reconciliation", "outcome_write_recovery"],
                true,
            ),
            (
                "downstream idempotency",
                20,
                &["one_effect_after_retry"],
                true,
            ),
            ("tenant isolation", 15, &["tenant_isolation"], true),
        ],
        Scenario::WorkerLifecycle => &[
            ("task timestamps", 20, &["task_timestamps"], true),
            ("task liveness", 35, &["task_reaping"], true),
            ("due worker ticks", 30, &["due_ticks"], true),
            ("polling cadence", 15, &["polling_clock"], true),
        ],
        Scenario::DeadlineSafety => &[
            ("dedup expiry", 20, &["dedup_boundary"], true),
            ("approval expiry", 25, &["approval_boundary"], true),
            ("lease expiry", 20, &["lease_boundary"], true),
            ("execution deadline", 20, &["timeout_boundary"], true),
            ("scheduled recovery", 15, &["retry_schedule"], false),
        ],
        _ => &[],
    };
    definitions
        .iter()
        .map(|&(name, weight, checks, safety)| Dimension {
            name,
            weight,
            checks,
            safety,
        })
        .collect()
}

/// Grade by unique named evidence. Absent or duplicate checks fail closed.
pub fn grade(scenario: Scenario, report: &ScenarioReport) -> Scorecard {
    let definitions = rubric(scenario);
    let evidence = |name: &str| {
        let mut matches = report
            .invariants
            .iter()
            .filter(|check| check.scenario == scenario && check.name == name);
        matches.next().is_some_and(|check| check.passed) && matches.next().is_none()
    };
    let dimensions: Vec<_> = definitions
        .iter()
        .map(|dimension| {
            let count = dimension
                .checks
                .iter()
                .filter(|name| evidence(name))
                .count();
            DimensionResult {
                name: dimension.name.into(),
                weight: dimension.weight,
                score: u32::try_from(count * 100 / dimension.checks.len()).expect("percentage"),
                checks: dimension.checks.iter().map(|name| (*name).into()).collect(),
            }
        })
        .collect();
    let mut gates = vec![GateResult {
        name: "complete_unique_evidence".into(),
        passed: !definitions.is_empty()
            && definitions.iter().all(|dimension| {
                dimension.checks.iter().all(|name| {
                    report
                        .invariants
                        .iter()
                        .filter(|check| check.scenario == scenario && check.name == *name)
                        .count()
                        == 1
                })
            }),
    }];
    gates.extend(
        definitions
            .iter()
            .filter(|dimension| dimension.safety)
            .flat_map(|dimension| dimension.checks.iter())
            .map(|name| GateResult {
                name: (*name).into(),
                passed: evidence(name),
            }),
    );
    Scorecard {
        passed: gates.iter().all(|gate| gate.passed)
            && report.passed()
            && dimensions.iter().all(|dimension| dimension.score == 100),
        score_basis_points: dimensions
            .iter()
            .map(|dimension| dimension.weight * dimension.score)
            .sum(),
        dimensions,
        gates,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrialResult {
    pub trial: u32,
    pub seed: u64,
    pub scenario: Scenario,
    pub scorecard: Scorecard,
    pub evidence: ScenarioReport,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub trials: usize,
    pub passed: usize,
    pub safety_gate_failures: usize,
    pub mean_score_basis_points: u32,
    pub worst_score_basis_points: u32,
}

fn summarize(results: &[TrialResult]) -> Summary {
    let total: u64 = results
        .iter()
        .map(|trial| u64::from(trial.scorecard.score_basis_points))
        .sum();
    Summary {
        trials: results.len(),
        passed: results
            .iter()
            .filter(|trial| trial.scorecard.passed)
            .count(),
        safety_gate_failures: results
            .iter()
            .filter(|trial| trial.scorecard.gates.iter().any(|gate| !gate.passed))
            .count(),
        mean_score_basis_points: u32::try_from(
            total / u64::try_from(results.len().max(1)).expect("trial count fits"),
        )
        .expect("mean score fits"),
        worst_score_basis_points: results
            .iter()
            .map(|trial| trial.scorecard.score_basis_points)
            .min()
            .unwrap_or(0),
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationReport {
    pub schema_version: u32,
    pub manifest: EvaluationManifest,
    pub manifest_sha256: String,
    pub provenance: Provenance,
    pub summary: Summary,
    pub results: Vec<TrialResult>,
}

impl EvaluationReport {
    pub fn consistent(&self) -> bool {
        self.schema_version == 2
            && self.manifest.validate().is_ok()
            && self.provenance.clock == self.manifest.clock_description()
            && self.provenance.grader_version == GRADER_VERSION
            && self.manifest_sha256
                == format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&self.manifest).expect("manifest serializes")
                    )
                )
            && self.results.len() == self.manifest.scenarios.len() * self.manifest.trials as usize
            && self.summary == summarize(&self.results)
            && !self.results.is_empty()
            && self.results.iter().enumerate().all(|(index, trial)| {
                let expected_trial =
                    u32::try_from(index / self.manifest.scenarios.len()).expect("bounded trials");
                let expected_scenario =
                    self.manifest.scenarios[index % self.manifest.scenarios.len()];
                trial.trial == expected_trial
                    && trial.scenario == expected_scenario
                    && trial.seed
                        == derived_seed(
                            self.manifest.seed,
                            expected_trial,
                            scenario_id(expected_scenario),
                        )
                    && trial.evidence.schema_version == 1
                    && trial.evidence.manifest
                        == (ScenarioManifest {
                            schema_version: 1,
                            seed: trial.seed,
                            backend: self.manifest.backend,
                            scenarios: vec![expected_scenario],
                        })
                    && trial.evidence.manifest_sha256
                        == format!(
                            "{:x}",
                            Sha256::digest(
                                serde_json::to_vec(&trial.evidence.manifest)
                                    .expect("manifest serializes")
                            )
                        )
                    && trial
                        .evidence
                        .trace
                        .iter()
                        .enumerate()
                        .all(|(index, event)| {
                            event.sequence == index
                                && event.scenario == expected_scenario
                                && event.caused_by == index.checked_sub(1)
                        })
                    && trial
                        .evidence
                        .invariants
                        .iter()
                        .all(|check| check.scenario == expected_scenario)
                    && trial.scorecard == grade(trial.scenario, &trial.evidence)
            })
    }

    pub fn passed(&self) -> bool {
        self.consistent() && self.results.iter().all(|trial| trial.scorecard.passed)
    }

    pub fn same_evidence(&self, previous: &Self) -> bool {
        self.consistent()
            && previous.consistent()
            && self.schema_version == previous.schema_version
            && self.manifest == previous.manifest
            && self.manifest_sha256 == previous.manifest_sha256
            && self.provenance == previous.provenance
            && self.summary == previous.summary
            && self.results.len() == previous.results.len()
            && self
                .results
                .iter()
                .zip(&previous.results)
                .all(|(new, old)| {
                    new.trial == old.trial
                        && new.seed == old.seed
                        && new.scenario == old.scenario
                        && new.scorecard == old.scorecard
                        && new.evidence.same_evidence(&old.evidence)
                })
    }

    pub fn junit(&self) -> String {
        use std::fmt::Write as _;
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite name=\"acteon-evaluation\" tests=\"{}\" failures=\"{}\">",
            self.results.len(),
            self.results
                .iter()
                .filter(|trial| !trial.scorecard.passed)
                .count()
        );
        for trial in &self.results {
            let _ = write!(
                xml,
                "<testcase classname=\"{}\" name=\"trial-{}-seed-{}\">",
                scenario_id(trial.scenario),
                trial.trial,
                trial.seed
            );
            if !trial.scorecard.passed {
                let failures: Vec<_> = trial
                    .evidence
                    .invariants
                    .iter()
                    .filter(|check| !check.passed)
                    .map(|check| format!("{}: {}", check.name, check.detail))
                    .collect();
                let _ = write!(
                    xml,
                    "<failure message=\"scenario evidence failed\">{}</failure>",
                    escape_xml(&failures.join("; "))
                );
            }
            xml.push_str("</testcase>");
        }
        xml.push_str("</testsuite>");
        xml
    }
}

pub async fn run(manifest: EvaluationManifest) -> Result<EvaluationReport, SimulationError> {
    manifest.validate()?;
    let provenance = Provenance::for_manifest(&manifest)?;
    let manifest_sha256 = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&manifest).expect("manifest serializes"))
    );
    let mut results = Vec::new();
    for trial in 0..manifest.trials {
        for scenario in &manifest.scenarios {
            let seed = derived_seed(manifest.seed, trial, scenario_id(*scenario));
            let evidence = super::run(ScenarioManifest {
                schema_version: 1,
                seed,
                backend: manifest.backend,
                scenarios: vec![*scenario],
            })
            .await?;
            results.push(TrialResult {
                trial,
                seed,
                scenario: *scenario,
                scorecard: grade(*scenario, &evidence),
                evidence,
            });
        }
    }
    Ok(EvaluationReport {
        schema_version: 2,
        manifest,
        manifest_sha256,
        provenance,
        summary: summarize(&results),
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> EvaluationManifest {
        EvaluationManifest {
            schema_version: 2,
            seed: 4_815_162_342,
            backend: Backend::Memory,
            trials: 2,
            scenarios: vec![
                Scenario::IncidentResponse,
                Scenario::RefundFulfillment,
                Scenario::PromptInjection,
            ],
        }
    }

    #[test]
    fn malformed_and_unbounded_manifests_are_rejected() {
        for trials in [0, MAX_TRIALS + 1, u32::MAX] {
            let mut candidate = manifest();
            candidate.trials = trials;
            assert!(candidate.validate().is_err());
        }
        let mut candidate = manifest();
        candidate.scenarios.push(Scenario::IncidentResponse);
        assert!(candidate.validate().is_err());
        candidate.scenarios = vec![Scenario::GeneratedPolicy];
        assert!(candidate.validate().is_err());
        candidate.scenarios.clear();
        assert!(candidate.validate().is_err());
        let mut encoded = serde_json::to_value(manifest()).unwrap();
        encoded["ignore_safety"] = true.into();
        assert!(serde_json::from_value::<EvaluationManifest>(encoded).is_err());
    }

    #[tokio::test]
    async fn portfolio_trials_pass_replay_and_reject_tampering() {
        let first = run(manifest()).await.unwrap();
        assert!(
            first.passed(),
            "{}",
            serde_json::to_string_pretty(&first).unwrap()
        );
        assert_eq!(first.summary.trials, 6);
        assert_eq!(first.summary.worst_score_basis_points, 10000);
        let replay = run(manifest()).await.unwrap();
        assert!(replay.same_evidence(&first));
        let mut reordered = manifest();
        reordered.scenarios.reverse();
        let reordered = run(reordered).await.unwrap();
        assert!(reordered.passed());
        for trial in &first.results {
            let counterpart = reordered
                .results
                .iter()
                .find(|candidate| {
                    candidate.trial == trial.trial && candidate.scenario == trial.scenario
                })
                .unwrap();
            assert_eq!(trial.seed, counterpart.seed);
            assert!(trial.evidence.same_evidence(&counterpart.evidence));
        }
        assert!(first.junit().contains("tests=\"6\" failures=\"0\""));
        for mutation in [
            "score",
            "identity",
            "seed",
            "missing_trial",
            "manifest_hash",
            "gate",
        ] {
            let mut value = serde_json::to_value(&first).unwrap();
            match mutation {
                "score" => value["results"][0]["scorecard"]["score_basis_points"] = 0.into(),
                "identity" => value["results"][0]["trial"] = 1.into(),
                "seed" => value["results"][0]["seed"] = 0.into(),
                "missing_trial" => {
                    value["results"].as_array_mut().unwrap().pop();
                }
                "manifest_hash" => value["manifest_sha256"] = "forged".into(),
                "gate" => value["results"][0]["scorecard"]["gates"] = serde_json::json!([]),
                _ => unreachable!(),
            }
            let changed: EvaluationReport = serde_json::from_value(value).unwrap();
            assert!(!changed.consistent(), "accepted {mutation}");
            assert!(!changed.passed());
            assert!(!replay.same_evidence(&changed));
        }
        let mut changed = first;
        changed.provenance.runner_sha256 = "different executable".into();
        assert!(!replay.same_evidence(&changed));
    }

    #[tokio::test]
    async fn missing_or_duplicate_evidence_cannot_pass_a_grade() {
        let mut report = super::super::run(ScenarioManifest {
            schema_version: 1,
            seed: 5,
            backend: Backend::Memory,
            scenarios: vec![Scenario::PromptInjection],
        })
        .await
        .unwrap();
        assert!(grade(Scenario::PromptInjection, &report).passed);
        let removed = report.invariants.remove(0);
        assert!(!grade(Scenario::PromptInjection, &report).passed);
        report.invariants.push(removed.clone());
        report.invariants.push(removed);
        assert!(!grade(Scenario::PromptInjection, &report).passed);
    }

    #[test]
    fn seed_streams_are_named_and_order_independent() {
        let incident = derived_seed(42, 1, "incident_response");
        assert_eq!(incident, 13_606_397_729_056_201_345);
        assert_ne!(incident, derived_seed(42, 0, "incident_response"));
        assert_ne!(incident, derived_seed(42, 1, "refund_fulfillment"));
        for scenario in manifest().scenarios {
            assert_eq!(
                rubric(scenario)
                    .iter()
                    .map(|dimension| dimension.weight)
                    .sum::<u32>(),
                100
            );
        }
    }
}
