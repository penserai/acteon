use std::process::Stdio;

use chrono::Utc;

use crate::config::{AdversarialConfig, AgentEngine, SwarmConfig};
use crate::error::SwarmError;
use crate::types::adversarial::{
    AdversarialChallenge, AdversarialResult, AdversarialRound, ChallengeSeverity,
};
use crate::types::plan::SwarmPlan;
use crate::types::run::SwarmRun;

/// Run the adversarial challenge-recovery loop.
///
/// Each round:
/// 1. **Challenge phase** — adversarial agents critique the primary swarm's output.
/// 2. **Filter** — only challenges above `severity_threshold` trigger recovery.
/// 3. **Recovery phase** — the primary engine's agents address actionable challenges.
/// 4. **Check** — if all challenges are resolved (or below threshold), stop early.
pub async fn run_adversarial_loop(
    config: &SwarmConfig,
    plan: &SwarmPlan,
    run: &mut SwarmRun,
    primary_output_summary: &str,
) -> Result<AdversarialResult, SwarmError> {
    let adv = &config.adversarial;
    let primary_engine = config.defaults.engine;
    let adversarial_engine = adv.effective_engine(primary_engine);

    tracing::info!(
        primary = ?primary_engine,
        adversarial = ?adversarial_engine,
        max_rounds = adv.max_rounds,
        threshold = adv.severity_threshold,
        "starting adversarial loop"
    );

    run.status = crate::types::run::SwarmRunStatus::Adversarial;
    let mut rounds = Vec::new();
    let mut cumulative_context = primary_output_summary.to_string();

    for round_num in 1..=adv.max_rounds {
        let round_result = execute_adversarial_round(
            adv,
            primary_engine,
            adversarial_engine,
            plan,
            run,
            &mut cumulative_context,
            round_num,
        )
        .await?;

        let stop = round_result.fully_resolved() || round_result.actionable_challenges == 0;
        rounds.push(round_result);

        if stop {
            break;
        }
    }

    let result = AdversarialResult::from_rounds(rounds, adv.severity_threshold);
    tracing::info!(
        accepted = result.accepted,
        total_challenges = result.total_challenges,
        total_resolved = result.total_resolved,
        unresolved = result.unresolved.len(),
        "adversarial loop complete"
    );

    Ok(result)
}

/// Execute a single adversarial challenge-recovery round.
async fn execute_adversarial_round(
    adv: &AdversarialConfig,
    primary_engine: AgentEngine,
    adversarial_engine: AgentEngine,
    plan: &SwarmPlan,
    run: &mut SwarmRun,
    cumulative_context: &mut String,
    round_num: usize,
) -> Result<AdversarialRound, SwarmError> {
    tracing::info!(round = round_num, "adversarial round starting");
    let challenge_started_at = Utc::now();

    // ── Challenge phase ────────────────────────────────────────────────────
    let mut challenges =
        run_challenge_phase(adv, adversarial_engine, plan, cumulative_context, round_num).await?;

    let actionable_count = challenges
        .iter()
        .filter(|c| c.severity_score >= adv.severity_threshold)
        .count();

    run.metrics.challenges_raised += challenges.len() as u64;
    tracing::info!(
        round = round_num,
        total = challenges.len(),
        actionable = actionable_count,
        "challenge phase complete"
    );

    if actionable_count == 0 {
        tracing::info!(round = round_num, "no actionable challenges");
        run.metrics.adversarial_rounds += 1;
        return Ok(AdversarialRound {
            round: round_num,
            challenge_engine: adversarial_engine,
            recovery_engine: primary_engine,
            challenges,
            actionable_challenges: 0,
            resolved_count: 0,
            challenge_started_at,
            recovery_finished_at: Some(Utc::now()),
        });
    }

    // ── Recovery phase ─────────────────────────────────────────────────────
    let recovery_summary = run_recovery_phase(
        adv,
        primary_engine,
        plan,
        &challenges,
        cumulative_context,
        round_num,
    )
    .await?;

    let resolved_count =
        mark_resolved_challenges(&mut challenges, &recovery_summary, adv.severity_threshold);
    run.metrics.challenges_resolved += resolved_count as u64;
    run.metrics.adversarial_rounds += 1;

    tracing::info!(
        round = round_num,
        resolved = resolved_count,
        actionable = actionable_count,
        "recovery phase complete"
    );

    *cumulative_context = format!(
        "{cumulative_context}\n\n## Adversarial Round {round_num} Recovery\n{recovery_summary}"
    );

    Ok(AdversarialRound {
        round: round_num,
        challenge_engine: adversarial_engine,
        recovery_engine: primary_engine,
        challenges,
        actionable_challenges: actionable_count,
        resolved_count,
        challenge_started_at,
        recovery_finished_at: Some(Utc::now()),
    })
}

/// Run the challenge phase: spawn adversarial agents to critique the primary output.
async fn run_challenge_phase(
    adv: &AdversarialConfig,
    engine: AgentEngine,
    plan: &SwarmPlan,
    primary_output: &str,
    round: usize,
) -> Result<Vec<AdversarialChallenge>, SwarmError> {
    let truncated: String = primary_output.chars().take(8000).collect();
    let task_summary = plan
        .tasks
        .iter()
        .map(|t| format!("- {} ({}): {}", t.id, t.assigned_role, t.description))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        r#"You are an adversarial reviewer for a multi-agent swarm (round {round}). Your job is to find flaws, risks, and gaps in the swarm's output.

## Plan Objective
{objective}

## Tasks Executed
{task_summary}

## Primary Swarm Output (truncated)
{truncated}

## Instructions
Critically review the output. Look for:
1. **Correctness** — Logic errors, missing edge cases, wrong behavior
2. **Security** — Injection vectors, unsafe operations, exposed internals
3. **Performance** — N+1 queries, unbounded allocations, missing caching
4. **Completeness** — Missing features, untested paths, incomplete implementations
5. **Style** — Code quality issues, naming problems, missing documentation

For each issue found, output a JSON object on its own line with these fields:
- "id": unique string (e.g., "c-{round}-1")
- "target_task_id": task ID or null if cross-cutting
- "category": one of "correctness", "security", "performance", "completeness", "style"
- "description": clear description of the issue
- "severity": one of "low", "medium", "high", "critical"
- "severity_score": float 0.0-1.0
- "suggested_fix": suggested remediation or null

Output ONLY JSON lines, one per issue. If no issues are found, output a single line: {{"id": "none", "category": "none", "description": "No issues found", "severity": "low", "severity_score": 0.0}}"#,
        objective = plan.objective,
    );

    let raw_output = invoke_engine(engine, &prompt, adv.challenge_timeout_seconds).await?;
    Ok(parse_challenges(&raw_output, round))
}

/// Run the recovery phase: spawn primary-engine agents to address challenges.
async fn run_recovery_phase(
    adv: &AdversarialConfig,
    engine: AgentEngine,
    plan: &SwarmPlan,
    challenges: &[AdversarialChallenge],
    primary_output: &str,
    round: usize,
) -> Result<String, SwarmError> {
    let actionable: Vec<&AdversarialChallenge> = challenges
        .iter()
        .filter(|c| c.severity_score >= adv.severity_threshold)
        .collect();

    if actionable.is_empty() {
        return Ok(String::new());
    }

    let challenge_list = actionable
        .iter()
        .map(|c| {
            let fix = c
                .suggested_fix
                .as_deref()
                .unwrap_or("no suggestion provided");
            format!(
                "- [{id}] ({severity:?}, {cat}): {desc}\n  Suggested fix: {fix}",
                id = c.id,
                severity = c.severity,
                cat = c.category,
                desc = c.description,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let truncated_output: String = primary_output.chars().take(6000).collect();

    let prompt = format!(
        r"You are a recovery agent for a multi-agent swarm (round {round}). The adversarial review found issues that need to be addressed.

## Plan Objective
{objective}

## Issues to Address
{challenge_list}

## Current State (truncated)
{truncated_output}

## Instructions
Address each issue listed above. For each:
1. Analyze the root cause
2. Describe the fix you would apply
3. Explain why the fix resolves the issue

Output a recovery report. For each addressed issue, include a line:
RESOLVED: <challenge-id> — <brief explanation of what was fixed>

If an issue cannot be resolved, include:
UNRESOLVED: <challenge-id> — <reason why it cannot be fixed>

Be thorough but concise.",
        objective = plan.objective,
    );

    invoke_engine(engine, &prompt, adv.recovery_timeout_seconds).await
}

/// Invoke an AI engine with a prompt and return the raw text output.
async fn invoke_engine(
    engine: AgentEngine,
    prompt: &str,
    timeout_seconds: u64,
) -> Result<String, SwarmError> {
    let cmd_name = match engine {
        AgentEngine::Claude => "claude",
        AgentEngine::Gemini => "gemini",
    };

    let mut cmd = tokio::process::Command::new(cmd_name);
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");

    match engine {
        AgentEngine::Claude => {
            cmd.arg("--model").arg("sonnet");
        }
        AgentEngine::Gemini => {
            cmd.arg("--yolo");
        }
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_seconds),
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(SwarmError::AdversarialChallenge(format!(
                "{cmd_name} exited with status {}: {stderr}",
                output.status
            )))
        }
        Ok(Err(e)) => Err(SwarmError::AdversarialChallenge(format!(
            "failed to spawn {cmd_name}: {e}"
        ))),
        Err(_) => Err(SwarmError::AdversarialChallenge(format!(
            "{cmd_name} timed out after {timeout_seconds}s"
        ))),
    }
}

// ── Parsing helpers ────────────────────────────────────────────────────────────

/// Parse challenge JSON lines from the adversarial agent's raw output.
fn parse_challenges(raw: &str, round: usize) -> Vec<AdversarialChallenge> {
    let mut challenges = Vec::new();
    let mut counter = 1;

    // Try line-by-line JSON first.
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = parse_single_challenge(line, round, &mut counter) {
            challenges.push(c);
        }
    }

    // Fallback: try parsing as a JSON array.
    if challenges.is_empty()
        && let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(raw.trim())
    {
        for json in arr {
            if let Some(c) = challenge_from_json(&json, round, &mut counter) {
                challenges.push(c);
            }
        }
    }

    challenges
}

/// Try to parse a single JSON line into a challenge.
fn parse_single_challenge(
    line: &str,
    round: usize,
    counter: &mut usize,
) -> Option<AdversarialChallenge> {
    // Strip trailing commas (common in array-formatted output).
    let trimmed = line.strip_suffix(',').unwrap_or(line);
    let json = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    challenge_from_json(&json, round, counter)
}

/// Build an `AdversarialChallenge` from a JSON value.
fn challenge_from_json(
    json: &serde_json::Value,
    round: usize,
    counter: &mut usize,
) -> Option<AdversarialChallenge> {
    // Skip the "no issues found" sentinel.
    if json
        .get("category")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|c| c == "none")
    {
        return None;
    }

    let id = json
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| format!("c-{round}-{counter}"), String::from);

    let severity_score = json
        .get("severity_score")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.5);

    let severity = json
        .get("severity")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| serde_json::from_value(serde_json::Value::String(s.into())).ok())
        .unwrap_or_else(|| ChallengeSeverity::from_score(severity_score));

    *counter += 1;

    Some(AdversarialChallenge {
        id,
        target_task_id: json
            .get("target_task_id")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        category: json
            .get("category")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("general")
            .to_string(),
        description: json
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no description")
            .to_string(),
        severity,
        severity_score,
        suggested_fix: json
            .get("suggested_fix")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        resolved: false,
        resolution: None,
    })
}

/// Mark challenges as resolved based on the recovery output.
///
/// Looks for `RESOLVED: <id>` lines in the recovery summary.
fn mark_resolved_challenges(
    challenges: &mut [AdversarialChallenge],
    recovery_output: &str,
    severity_threshold: f64,
) -> usize {
    let mut resolved_count = 0;

    for line in recovery_output.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("RESOLVED:")
            .or_else(|| line.strip_prefix("RESOLVED "))
        {
            // Format: "RESOLVED: <id> — <explanation>" or "RESOLVED: <id> - <explanation>"
            // Split on " — " (em-dash) or " - " (spaced hyphen) to avoid breaking IDs with dashes.
            let (id_part, explanation) = rest
                .split_once(" \u{2014} ")
                .or_else(|| rest.split_once(" - "))
                .map(|(id, expl)| (id.trim(), Some(expl.trim().to_string())))
                .unwrap_or((rest.trim(), None));

            if let Some(challenge) = challenges
                .iter_mut()
                .find(|c| c.id == id_part && c.severity_score >= severity_threshold)
            {
                challenge.resolved = true;
                challenge.resolution = explanation;
                resolved_count += 1;
            }
        }
    }

    resolved_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_challenges_json_lines() {
        let raw = r#"
{"id": "c-1-1", "target_task_id": "task-1", "category": "correctness", "description": "Missing null check", "severity": "high", "severity_score": 0.75, "suggested_fix": "Add null check"}
{"id": "c-1-2", "target_task_id": null, "category": "performance", "description": "Unbounded allocation", "severity": "medium", "severity_score": 0.5}
"#;
        let challenges = parse_challenges(raw, 1);
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].id, "c-1-1");
        assert_eq!(challenges[0].category, "correctness");
        assert_eq!(challenges[0].severity, ChallengeSeverity::High);
        assert_eq!(challenges[1].id, "c-1-2");
        assert!(challenges[1].suggested_fix.is_none());
    }

    #[test]
    fn test_parse_challenges_json_array() {
        let raw = r#"[
            {"id": "c-1-1", "category": "security", "description": "SQL injection", "severity": "critical", "severity_score": 1.0},
            {"id": "c-1-2", "category": "style", "description": "Inconsistent naming", "severity": "low", "severity_score": 0.25}
        ]"#;
        let challenges = parse_challenges(raw, 1);
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].severity, ChallengeSeverity::Critical);
        assert_eq!(challenges[1].severity, ChallengeSeverity::Low);
    }

    #[test]
    fn test_parse_challenges_skips_none_sentinel() {
        let raw = r#"{"id": "none", "category": "none", "description": "No issues found", "severity": "low", "severity_score": 0.0}"#;
        let challenges = parse_challenges(raw, 1);
        assert!(challenges.is_empty());
    }

    #[test]
    fn test_parse_challenges_handles_garbage() {
        let raw = "Here are the issues I found:\nSome prose.\n\nMore prose.";
        let challenges = parse_challenges(raw, 1);
        assert!(challenges.is_empty());
    }

    #[test]
    fn test_parse_challenges_generates_ids() {
        let raw = r#"{"category": "correctness", "description": "Bug", "severity": "medium", "severity_score": 0.5}"#;
        let challenges = parse_challenges(raw, 3);
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].id, "c-3-1");
    }

    #[test]
    fn test_mark_resolved_challenges() {
        let mut challenges = vec![
            AdversarialChallenge {
                id: "c-1-1".into(),
                target_task_id: None,
                category: "correctness".into(),
                description: "Bug".into(),
                severity: ChallengeSeverity::High,
                severity_score: 0.75,
                suggested_fix: None,
                resolved: false,
                resolution: None,
            },
            AdversarialChallenge {
                id: "c-1-2".into(),
                target_task_id: None,
                category: "style".into(),
                description: "Naming".into(),
                severity: ChallengeSeverity::Low,
                severity_score: 0.25,
                suggested_fix: None,
                resolved: false,
                resolution: None,
            },
        ];

        let recovery = "RESOLVED: c-1-1 \u{2014} Added null check to fix the bug\nUNRESOLVED: c-1-2 \u{2014} Style preference, not a blocker"; // Note: em-dash with spaces

        let count = mark_resolved_challenges(&mut challenges, recovery, 0.5);
        assert_eq!(count, 1);
        assert!(challenges[0].resolved);
        assert_eq!(
            challenges[0].resolution.as_deref(),
            Some("Added null check to fix the bug")
        );
        assert!(!challenges[1].resolved); // Below threshold, not marked.
    }

    #[test]
    fn test_mark_resolved_with_dash_separator() {
        let mut challenges = vec![AdversarialChallenge {
            id: "c-1-1".into(),
            target_task_id: None,
            category: "correctness".into(),
            description: "Bug".into(),
            severity: ChallengeSeverity::High,
            severity_score: 0.75,
            suggested_fix: None,
            resolved: false,
            resolution: None,
        }];

        let recovery = "RESOLVED: c-1-1 - Fixed the issue";
        let count = mark_resolved_challenges(&mut challenges, recovery, 0.5);
        assert_eq!(count, 1);
        assert!(challenges[0].resolved);
    }
}
