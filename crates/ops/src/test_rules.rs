//! Rule test-suite runner.
//!
//! Loads YAML test-fixture files and runs each case against the rule
//! evaluation endpoint, comparing expected vs actual verdict/matched-rule.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{OpsClient, OpsError};

// ---------------------------------------------------------------------------
// Fixture types (deserialized from YAML)
// ---------------------------------------------------------------------------

/// A YAML fixture file containing rule test cases.
#[derive(Debug, Clone, Deserialize)]
pub struct TestFixtureFile {
    /// The test cases to execute.
    pub tests: Vec<TestCase>,
}

/// A single test case within a fixture file.
#[derive(Debug, Clone, Deserialize)]
pub struct TestCase {
    /// Human-readable test name.
    pub name: String,
    /// The action to evaluate.
    pub action: TestAction,
    /// Expected evaluation result.
    pub expect: TestExpect,
}

/// Action parameters sent to the rule evaluation endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct TestAction {
    pub namespace: String,
    pub tenant: String,
    pub provider: String,
    pub action_type: String,
    #[serde(default = "default_payload")]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_payload() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Expected verdict and optional matched rule name.
#[derive(Debug, Clone, Deserialize)]
pub struct TestExpect {
    /// Expected verdict string (e.g. `"allow"`, `"suppress"`, `"deny"`).
    pub verdict: String,
    /// If set, the name of the rule that should have matched.
    #[serde(default)]
    pub matched_rule: Option<String>,
}

// ---------------------------------------------------------------------------
// Result types (returned from the runner)
// ---------------------------------------------------------------------------

/// Result of running an entire test suite.
#[derive(Debug, Clone, Serialize)]
pub struct TestRunSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<TestCaseResult>,
    pub duration_ms: u64,
}

/// Result of a single test case.
#[derive(Debug, Clone, Serialize)]
pub struct TestCaseResult {
    pub name: String,
    pub passed: bool,
    pub expected_verdict: String,
    pub actual_verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_rule: Option<String>,
    pub duration_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Parse a YAML fixture file from a string.
pub fn parse_fixture(yaml: &str) -> Result<TestFixtureFile, OpsError> {
    serde_yaml_ng::from_str(yaml)
        .map_err(|e| OpsError::Configuration(format!("invalid fixture YAML: {e}")))
}

/// Run a full test suite against a live gateway.
///
/// Each test case is evaluated via the rule playground endpoint.
/// An optional `filter` restricts execution to test names containing the
/// given substring.
pub async fn run_test_suite(
    ops: &OpsClient,
    fixtures: &TestFixtureFile,
    filter: Option<&str>,
) -> Result<TestRunSummary, OpsError> {
    let cases: Vec<&TestCase> = fixtures
        .tests
        .iter()
        .filter(|tc| match filter {
            Some(f) => tc.name.contains(f),
            None => true,
        })
        .collect();

    let mut results = Vec::with_capacity(cases.len());
    let suite_start = Instant::now();

    for tc in &cases {
        let case_start = Instant::now();

        let eval_result = ops
            .evaluate_rules(
                tc.action.namespace.clone(),
                tc.action.tenant.clone(),
                tc.action.provider.clone(),
                tc.action.action_type.clone(),
                tc.action.payload.clone(),
                false,
            )
            .await;

        #[allow(clippy::cast_possible_truncation)]
        let duration_us = case_start.elapsed().as_micros() as u64;

        match eval_result {
            Ok(trace) => {
                let verdict_match = trace.verdict.eq_ignore_ascii_case(&tc.expect.verdict);

                let rule_match = match &tc.expect.matched_rule {
                    Some(expected) => trace
                        .matched_rule
                        .as_ref()
                        .is_some_and(|actual| actual == expected),
                    None => true, // not asserted
                };

                let passed = verdict_match && rule_match;

                results.push(TestCaseResult {
                    name: tc.name.clone(),
                    passed,
                    expected_verdict: tc.expect.verdict.clone(),
                    actual_verdict: trace.verdict.clone(),
                    expected_rule: tc.expect.matched_rule.clone(),
                    actual_rule: trace.matched_rule.clone(),
                    duration_us,
                    error: None,
                });
            }
            Err(e) => {
                results.push(TestCaseResult {
                    name: tc.name.clone(),
                    passed: false,
                    expected_verdict: tc.expect.verdict.clone(),
                    actual_verdict: String::new(),
                    expected_rule: tc.expect.matched_rule.clone(),
                    actual_rule: None,
                    duration_us,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.len() - passed;
    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = suite_start.elapsed().as_millis() as u64;

    Ok(TestRunSummary {
        total: results.len(),
        passed,
        failed,
        results,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_fixture() {
        let yaml = r#"
tests:
  - name: "allow normal email"
    action:
      namespace: notifications
      tenant: acme
      provider: email
      action_type: send_email
      payload: { to: "user@example.com" }
    expect:
      verdict: allow
"#;
        let fixture = parse_fixture(yaml).unwrap();
        assert_eq!(fixture.tests.len(), 1);
        assert_eq!(fixture.tests[0].name, "allow normal email");
        assert_eq!(fixture.tests[0].expect.verdict, "allow");
        assert!(fixture.tests[0].expect.matched_rule.is_none());
    }

    #[test]
    fn parse_fixture_with_matched_rule() {
        let yaml = r#"
tests:
  - name: "spam is suppressed"
    action:
      namespace: notifications
      tenant: acme
      provider: email
      action_type: spam
      payload: {}
    expect:
      verdict: suppress
      matched_rule: block-spam
"#;
        let fixture = parse_fixture(yaml).unwrap();
        assert_eq!(
            fixture.tests[0].expect.matched_rule.as_deref(),
            Some("block-spam")
        );
    }

    #[test]
    fn parse_fixture_default_payload() {
        let yaml = r#"
tests:
  - name: "empty payload"
    action:
      namespace: ns
      tenant: t
      provider: p
      action_type: at
    expect:
      verdict: allow
"#;
        let fixture = parse_fixture(yaml).unwrap();
        assert!(fixture.tests[0].action.payload.is_object());
    }

    #[test]
    fn parse_invalid_yaml() {
        let yaml = "not: valid: yaml: [[[";
        assert!(parse_fixture(yaml).is_err());
    }

    #[test]
    fn parse_multiple_cases() {
        let yaml = r#"
tests:
  - name: "case 1"
    action:
      namespace: ns
      tenant: t
      provider: p
      action_type: a
      payload: {}
    expect:
      verdict: allow
  - name: "case 2"
    action:
      namespace: ns
      tenant: t
      provider: p
      action_type: b
      payload: {}
    expect:
      verdict: deny
      matched_rule: deny-b
"#;
        let fixture = parse_fixture(yaml).unwrap();
        assert_eq!(fixture.tests.len(), 2);
        assert_eq!(fixture.tests[1].expect.verdict, "deny");
    }
}
