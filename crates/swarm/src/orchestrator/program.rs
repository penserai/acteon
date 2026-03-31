use std::path::Path;

use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::types::plan::SwarmPlan;

/// Generate a `program.md` constraint document from the plan and config.
///
/// This is the "untouchable rules" document inspired by Karpathy's autoresearch
/// `program.md`. Agents read it but must not modify it. It defines the objective,
/// constraints, eval harness, and inviolable rules.
pub fn generate_program_md(
    plan: &SwarmPlan,
    config: &SwarmConfig,
    baseline_score: Option<f64>,
) -> String {
    let mut sections = Vec::new();

    // Header.
    sections.push("# Program Constraints\n\nThis file defines the rules and constraints for this swarm run. **Do NOT modify this file.**".to_string());

    // Objective.
    sections.push(format!("## Objective\n\n{}", plan.objective));

    // Success criteria.
    if !plan.success_criteria.is_empty() {
        let criteria = plan
            .success_criteria
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Success Criteria\n\n{criteria}"));
    }

    // Scope constraints.
    let mut scope_lines = vec![format!(
        "- Working directory: `{}`",
        plan.scope.working_directory.display()
    )];
    if !plan.scope.allowed_paths.is_empty() {
        let paths = plan
            .scope
            .allowed_paths
            .iter()
            .map(|p| format!("`{}`", p.display()))
            .collect::<Vec<_>>()
            .join(", ");
        scope_lines.push(format!("- Allowed paths: {paths}"));
    }
    if !plan.scope.forbidden_patterns.is_empty() {
        let patterns = plan
            .scope
            .forbidden_patterns
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ");
        scope_lines.push(format!("- Forbidden patterns: {patterns}"));
    }
    scope_lines.push(format!(
        "- Max concurrent agents: {}",
        plan.scope.max_agents
    ));
    scope_lines.push(format!(
        "- Max duration: {} minutes",
        plan.scope.max_duration_minutes
    ));
    sections.push(format!("## Scope\n\n{}", scope_lines.join("\n")));

    // Eval harness.
    if config.eval_harness.enabled && !config.eval_harness.command.is_empty() {
        let score_line = baseline_score.map_or_else(
            || "- Baseline score: not yet measured".to_string(),
            |s| format!("- Baseline score: {s:.2}"),
        );
        sections.push(format!(
            "## Eval Harness\n\n- Command: `{cmd}`\n- Pass threshold: {threshold}\n{score_line}\n- Timeout: {timeout}s\n\nThe eval command produces a fitness score. Changes that reduce the score will be **automatically reverted**.",
            cmd = config.eval_harness.command,
            threshold = config.eval_harness.pass_threshold,
            timeout = config.eval_harness.timeout_seconds,
        ));
    }

    // Task overview.
    if !plan.tasks.is_empty() {
        let task_lines = plan
            .tasks
            .iter()
            .map(|t| {
                let deps = if t.depends_on.is_empty() {
                    String::new()
                } else {
                    format!(" (depends: {})", t.depends_on.join(", "))
                };
                format!("- **{}** ({}): {}{deps}", t.id, t.assigned_role, t.name)
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Tasks\n\n{task_lines}"));
    }

    // Inviolable rules.
    sections.push(
        r"## Inviolable Rules

1. Do NOT modify the eval command, eval script, or test infrastructure
2. Do NOT delete or rename test files
3. Do NOT disable, skip, or comment out existing tests
4. Do NOT modify this file (`program.md`)
5. Do NOT modify `.claude/settings.json` or `.gemini/settings.json`
6. Keep all changes within the working directory
7. Do NOT introduce dependencies not already in the project without justification
8. Every change must preserve or improve the eval score"
            .to_string(),
    );

    sections.join("\n\n")
}

/// Write the program.md file to the workspace.
pub async fn write_program_md(working_dir: &Path, content: &str) -> Result<(), SwarmError> {
    let path = working_dir.join("program.md");
    tokio::fs::write(&path, content).await.map_err(|e| {
        SwarmError::EvalHarness(format!(
            "failed to write program.md to {}: {e}",
            path.display()
        ))
    })?;
    tracing::info!(path = %path.display(), "program.md written");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::plan::{SwarmScope, SwarmTask};

    fn sample_plan() -> SwarmPlan {
        SwarmPlan {
            id: "test-plan".into(),
            objective: "Build a REST API".into(),
            scope: SwarmScope {
                working_directory: "/tmp/project".into(),
                forbidden_patterns: vec!["*.env".into()],
                max_agents: 3,
                max_duration_minutes: 30,
                ..Default::default()
            },
            success_criteria: vec!["All tests pass".into(), "No clippy warnings".into()],
            tasks: vec![SwarmTask {
                id: "task-1".into(),
                name: "Scaffold project".into(),
                description: "Create initial structure".into(),
                assigned_role: "coder".into(),
                subtasks: vec![],
                depends_on: vec![],
                priority: 1,
            }],
            agent_roles: vec!["coder".into()],
            estimated_actions: 50,
            created_at: chrono::Utc::now(),
            approved_at: None,
        }
    }

    #[test]
    fn test_generate_program_md_includes_all_sections() {
        let plan = sample_plan();
        let config = SwarmConfig::minimal();
        let md = generate_program_md(&plan, &config, None);

        assert!(md.contains("# Program Constraints"));
        assert!(md.contains("Build a REST API"));
        assert!(md.contains("All tests pass"));
        assert!(md.contains("No clippy warnings"));
        assert!(md.contains("/tmp/project"));
        assert!(md.contains("`*.env`"));
        assert!(md.contains("Inviolable Rules"));
        assert!(md.contains("task-1"));
    }

    #[test]
    fn test_generate_with_baseline_score() {
        let plan = sample_plan();
        let mut config = SwarmConfig::minimal();
        config.eval_harness.enabled = true;
        config.eval_harness.command = "cargo test".into();
        let md = generate_program_md(&plan, &config, Some(0.85));

        assert!(md.contains("Baseline score: 0.85"));
    }

    #[test]
    fn test_generate_without_baseline_score() {
        let plan = sample_plan();
        let config = SwarmConfig::minimal();
        let md = generate_program_md(&plan, &config, None);

        // No eval harness section when not enabled.
        assert!(!md.contains("Eval Harness"));
    }

    #[test]
    fn test_generate_with_eval_harness() {
        let plan = sample_plan();
        let mut config = SwarmConfig::minimal();
        config.eval_harness.enabled = true;
        config.eval_harness.command = "cargo test".into();
        config.eval_harness.pass_threshold = 0.7;

        let md = generate_program_md(&plan, &config, Some(0.9));
        assert!(md.contains("## Eval Harness"));
        assert!(md.contains("`cargo test`"));
        assert!(md.contains("0.7"));
        assert!(md.contains("0.90"));
        assert!(md.contains("automatically reverted"));
    }
}
