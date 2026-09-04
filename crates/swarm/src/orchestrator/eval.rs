use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::EvalHarnessConfig;
use crate::error::SwarmError;
use crate::types::eval::EvalResult;

/// Versioned result emitted by an operator-controlled, independent evaluator.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalReport {
    pub schema_version: u32,
    pub checks: Vec<EvalCheck>,
}

/// One evidence-producing check. Safety checks are hard gates by default.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCheck {
    pub id: String,
    pub passed: bool,
    #[serde(default = "default_hard_gate")]
    pub hard_gate: bool,
    #[serde(default)]
    pub challenge_id: Option<String>,
}

const fn default_hard_gate() -> bool {
    true
}

fn invalid(message: impl Into<String>) -> SwarmError {
    SwarmError::EvalHarness(message.into())
}

/// Run a trusted evaluator. Unparseable output, process failure, and missing checks fail closed.
pub async fn run_eval_harness(
    config: &EvalHarnessConfig,
    working_dir: &Path,
) -> Result<EvalResult, SwarmError> {
    run_eval_with_challenges(config, working_dir, &[]).await
}

pub(crate) async fn run_eval_with_challenges(
    config: &EvalHarnessConfig,
    working_dir: &Path,
    challenges: &[crate::types::adversarial::AdversarialChallenge],
) -> Result<EvalResult, SwarmError> {
    if !config.pass_threshold.is_finite() || !(0.0..=1.0).contains(&config.pass_threshold) {
        return Err(invalid("pass_threshold must be finite and between 0 and 1"));
    }
    let mut command = match &config.program {
        Some(program) if !program.trim().is_empty() && config.command.trim().is_empty() => {
            let mut command = tokio::process::Command::new(program);
            command.args(&config.args);
            command
        }
        None if !config.command.trim().is_empty() && config.args.is_empty() => {
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args(["-c", &config.command]);
            command
        }
        _ => {
            return Err(invalid(
                "configure exactly one evaluator program/args or legacy command",
            ));
        }
    };
    command
        .current_dir(working_dir)
        .env("ACTEON_EVAL_CHALLENGES", serde_json::to_string(challenges)?);
    let start = std::time::Instant::now();
    let output = super::process::run(&mut command, Duration::from_secs(config.timeout_seconds))
        .await
        .map_err(|e| invalid(format!("evaluator process failed: {e}")))?;
    let exit_code = output.status.code().unwrap_or(-1);
    if !output.status.success() {
        return Err(invalid(format!("evaluator exited with status {exit_code}")));
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid("evaluator stdout is not UTF-8"))?;
    let (score, mut metrics, gates_passed, verified_challenges) =
        if stdout.trim_start().starts_with('{') {
            parse_report(stdout)?
        } else {
            let (score, metrics) = parse_eval_output(stdout)?;
            (score, metrics, true, Vec::new())
        };
    metrics.insert("exit_code".into(), f64::from(exit_code));
    let passed = gates_passed && score >= config.pass_threshold;
    Ok(EvalResult {
        score,
        passed,
        metrics,
        // Presentation truncation happens only after validation of complete stdout.
        output: format!("{stdout}\n{}", String::from_utf8_lossy(&output.stderr))
            .chars()
            .take(10000)
            .collect(),
        duration_seconds: start.elapsed().as_secs_f64(),
        exit_code,
        verified_challenges: if passed {
            verified_challenges
        } else {
            Vec::new()
        },
    })
}

type ParsedReport = (f64, HashMap<String, f64>, bool, Vec<String>);

fn parse_report(output: &str) -> Result<ParsedReport, SwarmError> {
    let report: EvalReport = serde_json::from_str(output)
        .map_err(|e| invalid(format!("invalid evaluator report: {e}")))?;
    if report.schema_version != 1 || report.checks.is_empty() {
        return Err(invalid(
            "evaluator report requires schema_version=1 and at least one check",
        ));
    }
    let mut ids = HashSet::new();
    let mut challenges: HashMap<String, bool> = HashMap::new();
    let mut passed_count = 0u32;
    let mut gates_passed = true;
    for check in &report.checks {
        if check.id.trim().is_empty() || !ids.insert(&check.id) {
            return Err(invalid("check IDs must be nonempty and unique"));
        }
        passed_count += u32::from(check.passed);
        gates_passed &= !check.hard_gate || check.passed;
        if let Some(id) = &check.challenge_id {
            if id.trim().is_empty() {
                return Err(invalid("empty challenge ID"));
            }
            *challenges.entry(id.clone()).or_insert(true) &= check.passed;
        }
    }
    let count = u32::try_from(report.checks.len()).map_err(|_| invalid("too many checks"))?;
    let score = f64::from(passed_count) / f64::from(count);
    let mut verified: Vec<_> = challenges
        .into_iter()
        .filter_map(|(id, passed)| passed.then_some(id))
        .collect();
    verified.sort();
    Ok((
        score,
        HashMap::from([
            ("test_count".into(), f64::from(count)),
            ("pass_count".into(), f64::from(passed_count)),
        ]),
        gates_passed,
        verified,
    ))
}

/// Strict compatibility parser for legacy, operator-authored evaluators.
/// Exactly one score signal is required; stderr and model prose are never graders.
fn parse_eval_output(output: &str) -> Result<(f64, HashMap<String, f64>), SwarmError> {
    let mut metrics = HashMap::new();
    let mut score = None;
    let mut warnings = None;
    for line in output.lines() {
        let upper = line.trim().to_uppercase();
        let parsed = if let Some(rest) = upper.strip_prefix("SCORE:") {
            let value: f64 = rest.trim().parse().map_err(|_| invalid("invalid SCORE"))?;
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(invalid("SCORE must be finite and between 0 and 1"));
            }
            Some(value)
        } else if let Some(rest) = upper.strip_prefix("PASS:") {
            let (n, total) = rest
                .trim()
                .split_once('/')
                .ok_or_else(|| invalid("invalid PASS"))?;
            let n: u32 = n
                .trim()
                .parse()
                .map_err(|_| invalid("invalid pass count"))?;
            let total: u32 = total
                .trim()
                .parse()
                .map_err(|_| invalid("invalid test count"))?;
            if total == 0 || n > total {
                return Err(invalid("invalid pass/test counts"));
            }
            metrics.insert("pass_count".into(), f64::from(n));
            metrics.insert("test_count".into(), f64::from(total));
            Some(f64::from(n) / f64::from(total))
        } else {
            if let Some(rest) = upper.strip_prefix("WARNINGS:") {
                let count: u32 = rest
                    .trim()
                    .parse()
                    .map_err(|_| invalid("invalid warning count"))?;
                if warnings.replace(count).is_some() {
                    return Err(invalid("duplicate warning count"));
                }
            }
            None
        };
        if let Some(parsed) = parsed
            && score.replace(parsed).is_some()
        {
            return Err(invalid("multiple score signals are ambiguous"));
        }
    }
    let score = score.ok_or_else(|| invalid("evaluator emitted no score"))?;
    let warnings = f64::from(warnings.unwrap_or(0));
    metrics.insert("warnings".into(), warnings);
    Ok(((score - warnings * 0.01).max(0.0), metrics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_and_ambiguous_scores_fail_closed() {
        for output in [
            "",
            "no score",
            "SCORE: nope",
            "SCORE: NaN",
            "SCORE: inf",
            "SCORE: 1.5",
            "SCORE: -0.1",
            "PASS: 0/0",
            "PASS: 2/1",
            "PASS: -1/2",
            "SCORE: 1\nWARNINGS: -100",
            "SCORE: 0\nSCORE: 1",
            "PASS: 1/1\nSCORE: 1",
        ] {
            assert!(parse_eval_output(output).is_err(), "accepted {output}");
        }
    }

    #[test]
    fn valid_legacy_scores_remain_supported() {
        assert!(
            (parse_eval_output("score: 0.9\nWARNINGS: 5").unwrap().0 - 0.85).abs() < f64::EPSILON
        );
        assert!((parse_eval_output("PASS: 42/50").unwrap().0 - 0.84).abs() < f64::EPSILON);
    }

    #[test]
    fn reports_require_unique_checks_and_honor_hard_gates() {
        for output in [
            r#"{"schema_version":1,"checks":[]}"#,
            r#"{"schema_version":2,"checks":[]}"#,
            r#"{"schema_version":1,"checks":[{"id":"a","passed":true},{"id":"a","passed":true}]}"#,
        ] {
            assert!(parse_report(output).is_err());
        }
        let (_, _, gates, verified) = parse_report(r#"{"schema_version":1,"checks":[{"id":"safety","passed":false,"challenge_id":"c1"},{"id":"quality","passed":true,"challenge_id":"c1"}]}"#).unwrap();
        assert!(!gates);
        assert!(verified.is_empty());
    }

    #[tokio::test]
    async fn failed_process_cannot_forge_a_passing_score() {
        let config = EvalHarnessConfig {
            command: "echo SCORE: 1; exit 1".into(),
            ..Default::default()
        };
        assert!(run_eval_harness(&config, Path::new(".")).await.is_err());
    }

    #[tokio::test]
    async fn score_is_parsed_before_display_truncation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("result"),
            format!("{}\nPASS: 0/1\n", "x".repeat(10001)),
        )
        .unwrap();
        let config = EvalHarnessConfig {
            program: Some("cat".into()),
            args: vec!["result".into()],
            ..Default::default()
        };
        let result = run_eval_harness(&config, dir.path()).await.unwrap();
        assert!(!result.passed);
        assert!(result.score.abs() < f64::EPSILON);
    }
}
