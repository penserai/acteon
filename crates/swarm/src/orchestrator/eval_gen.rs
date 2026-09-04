//! Typed regression plans. Model-authored text is never executed as a command.
//!
//! These checks verify regressions only. Challenge-specific acceptance must come
//! from an independently configured oracle, not heuristic code searches.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::eval::{EvalCheck, EvalReport};
use crate::error::SwarmError;

/// An operator-reviewed list of literal executable/argument pairs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalPlan {
    pub schema_version: u32,
    pub checks: Vec<CommandCheck>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCheck {
    pub id: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Generate baseline checks from known manifests, without calling a model.
#[must_use]
pub fn generate_regression_plan(working_dir: &Path) -> EvalPlan {
    let mut checks = Vec::new();
    for (marker, id, program, args) in [
        (
            "Cargo.toml",
            "rust-check",
            "cargo",
            vec!["check", "--locked", "--all-targets"],
        ),
        ("package.json", "node-tests", "npm", vec!["test"]),
        (
            "pyproject.toml",
            "python-tests",
            "python3",
            vec!["-m", "pytest"],
        ),
        ("go.mod", "go-tests", "go", vec!["test", "./..."]),
        ("pom.xml", "java-tests", "mvn", vec!["test"]),
    ] {
        if working_dir.join(marker).is_file() {
            checks.push(CommandCheck {
                id: id.into(),
                program: program.into(),
                args: args.into_iter().map(String::from).collect(),
            });
        }
    }
    EvalPlan {
        schema_version: 1,
        checks,
    }
}

/// Execute a trusted plan with bounded subprocesses and return a JSON-ready report.
pub async fn run_plan(
    plan: &EvalPlan,
    working_dir: &Path,
    timeout: Duration,
) -> Result<EvalReport, SwarmError> {
    let mut ids = HashSet::new();
    if plan.schema_version != 1 || plan.checks.is_empty() {
        return Err(SwarmError::EvalHarness(
            "regression plan requires schema_version=1 and nonempty checks".into(),
        ));
    }
    for check in &plan.checks {
        if check.id.trim().is_empty() || check.program.trim().is_empty() || !ids.insert(&check.id) {
            return Err(SwarmError::EvalHarness(
                "plan check IDs must be unique and commands nonempty".into(),
            ));
        }
    }
    let mut checks = Vec::new();
    for check in &plan.checks {
        let mut command = tokio::process::Command::new(&check.program);
        command.args(&check.args).current_dir(working_dir);
        let output = super::process::run(&mut command, timeout).await?;
        checks.push(EvalCheck {
            id: check.id.clone(),
            passed: output.status.success(),
            hard_gate: true,
            challenge_id: None,
        });
    }
    Ok(EvalReport {
        schema_version: 1,
        checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn assertion_names_and_arguments_are_literal_data() {
        let dir = tempfile::tempdir().unwrap();
        let plan = EvalPlan {
            schema_version: 1,
            checks: vec![CommandCheck {
                id: "$(touch injected); SCORE: 1".into(),
                program: "printf".into(),
                args: vec!["%s".into(), "$(touch injected)".into()],
            }],
        };
        let report = run_plan(&plan, dir.path(), Duration::from_secs(3))
            .await
            .unwrap();
        assert!(report.checks[0].passed);
        assert!(!dir.path().join("injected").exists());
        assert!(report.checks[0].challenge_id.is_none());
    }

    #[test]
    fn empty_workspace_never_generates_a_trivial_passing_check() {
        let dir = tempfile::tempdir().unwrap();
        assert!(generate_regression_plan(dir.path()).checks.is_empty());
    }
}
