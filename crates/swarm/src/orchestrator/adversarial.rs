use std::path::Path;
use std::process::Stdio;

use chrono::Utc;
use uuid::Uuid;

use crate::config::{AgentEngine, EvalHarnessConfig, RecoveryMode, SwarmConfig};
use crate::error::SwarmError;
use crate::orchestrator::agent_spawner::{spawn_agent, wait_for_agent};
use crate::orchestrator::eval;
use crate::orchestrator::eval_gen;
use crate::types::adversarial::{
    AdversarialChallenge, AdversarialResult, AdversarialRound, ChallengeSeverity,
};
use crate::types::agent::{AgentSession, AgentSessionStatus};
use crate::types::plan::{SwarmPlan, SwarmSubtask};
use crate::types::run::SwarmRun;

/// Bundled context for the adversarial loop to avoid wide function signatures.
pub struct AdversarialContext<'a> {
    pub config: &'a SwarmConfig,
    pub plan: &'a SwarmPlan,
    pub working_dir: &'a Path,
    pub hooks_binary: &'a Path,
    pub program_md: &'a str,
    pub eval_config: Option<&'a EvalHarnessConfig>,
    pub baseline_score: Option<f64>,
}

/// Run the adversarial challenge-recovery loop.
///
/// Each round:
/// 1. **Challenge phase** — adversarial agents critique the primary swarm's output.
/// 2. **Filter** — only challenges above `severity_threshold` trigger recovery.
/// 3. **Recovery phase** — agents address challenges (text analysis or code-writing).
/// 4. **Eval** — if eval harness is enabled, measure score and revert on regression.
/// 5. **Check** — if all challenges are resolved (or below threshold), stop early.
#[allow(clippy::too_many_lines)]
pub async fn run_adversarial_loop(
    ctx: &AdversarialContext<'_>,
    run: &mut SwarmRun,
    primary_output_summary: &str,
) -> Result<AdversarialResult, SwarmError> {
    let adv = &ctx.config.adversarial;
    let primary_engine = ctx.config.defaults.engine;
    let adversarial_engine = adv.effective_engine(primary_engine);

    tracing::info!(
        primary = ?primary_engine,
        adversarial = ?adversarial_engine,
        max_rounds = adv.max_rounds,
        threshold = adv.severity_threshold,
        recovery_mode = ?adv.recovery_mode,
        baseline_score = ?ctx.baseline_score,
        "starting adversarial loop"
    );

    run.status = crate::types::run::SwarmRunStatus::Adversarial;
    let mut rounds = Vec::new();
    let mut cumulative_context = primary_output_summary.to_string();
    let mut current_score = ctx.baseline_score;

    for round_num in 1..=adv.max_rounds {
        // Git snapshot before recovery (so we can revert if score regresses).
        let snapshot = if ctx.eval_config.is_some() {
            eval::git_snapshot(ctx.working_dir).await
        } else {
            String::new()
        };

        let mut round_result = execute_adversarial_round(
            ctx,
            primary_engine,
            adversarial_engine,
            run,
            &mut cumulative_context,
            round_num,
        )
        .await?;

        // Generate challenge-specific eval script from this round's challenges.
        if ctx.eval_config.is_some()
            && let Ok(script) = eval_gen::generate_eval_script(
                ctx.config,
                &round_result.challenges,
                ctx.working_dir,
                adv.severity_threshold,
            )
            .await
        {
            let script_path = ctx.working_dir.join("eval-harness.sh");
            if let Err(e) = tokio::fs::write(&script_path, &script).await {
                tracing::warn!("failed to write eval script: {e}");
            } else {
                tracing::info!(
                    assertions = round_result.challenges.len(),
                    "generated challenge-specific eval script"
                );
            }
        }

        // Eval gating: run eval after recovery, compare to previous score.
        // Use the generated eval script if available, fall back to static config.
        if let Some(eval_cfg) = ctx.eval_config {
            round_result.pre_eval_score = current_score;

            let generated_script = ctx.working_dir.join("eval-harness.sh");
            let effective_eval = if generated_script.exists() {
                EvalHarnessConfig {
                    enabled: true,
                    command: format!("sh {}", generated_script.display()),
                    timeout_seconds: eval_cfg.timeout_seconds,
                    pass_threshold: eval_cfg.pass_threshold,
                }
            } else {
                eval_cfg.clone()
            };

            match eval::run_eval_harness(&effective_eval, ctx.working_dir).await {
                Ok(eval_result) => {
                    let post_score = eval_result.score;
                    round_result.post_eval_score = Some(post_score);

                    if let Some(prev) = current_score {
                        if post_score < prev {
                            tracing::warn!(
                                round = round_num,
                                pre = prev,
                                post = post_score,
                                "eval score regressed — reverting recovery changes"
                            );
                            eval::git_revert_to_snapshot(ctx.working_dir, &snapshot).await;
                            round_result.post_eval_score = current_score;
                            rounds.push(round_result);
                            break;
                        }
                        tracing::info!(
                            round = round_num,
                            pre = prev,
                            post = post_score,
                            delta = post_score - prev,
                            "eval score after recovery"
                        );
                    }

                    eval::git_discard_snapshot(ctx.working_dir, &snapshot).await;
                    current_score = Some(post_score);
                }
                Err(e) => {
                    tracing::warn!(round = round_num, "eval harness failed: {e}");
                    eval::git_discard_snapshot(ctx.working_dir, &snapshot).await;
                }
            }
        }

        let stop = round_result.fully_resolved() || round_result.actionable_challenges == 0;
        rounds.push(round_result);

        if stop {
            break;
        }
    }

    // Update final eval score in metrics.
    if let Some(score) = current_score {
        run.metrics.eval_final_score = Some(score);
    }

    let result = AdversarialResult::from_rounds(rounds, adv.severity_threshold);
    tracing::info!(
        accepted = result.accepted,
        total_challenges = result.total_challenges,
        total_resolved = result.total_resolved,
        unresolved = result.unresolved.len(),
        final_score = ?current_score,
        "adversarial loop complete"
    );

    Ok(result)
}

/// Execute a single adversarial challenge-recovery round.
async fn execute_adversarial_round(
    ctx: &AdversarialContext<'_>,
    primary_engine: AgentEngine,
    adversarial_engine: AgentEngine,
    run: &mut SwarmRun,
    cumulative_context: &mut String,
    round_num: usize,
) -> Result<AdversarialRound, SwarmError> {
    let adv = &ctx.config.adversarial;

    tracing::info!(round = round_num, "adversarial round starting");
    let challenge_started_at = Utc::now();

    // ── Challenge phase ────────────────────────────────────────────────────
    let mut challenges = run_challenge_phase(
        adv,
        adversarial_engine,
        ctx.plan,
        cumulative_context,
        round_num,
    )
    .await?;

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
            pre_eval_score: None,
            post_eval_score: None,
        });
    }

    // ── Recovery phase ─────────────────────────────────────────────────────
    let recovery_summary = match adv.recovery_mode {
        RecoveryMode::Fix => run_recovery_agents(ctx, &challenges, run, round_num).await?,
        RecoveryMode::Analyze => {
            run_recovery_analyze(
                adv,
                primary_engine,
                ctx.plan,
                &challenges,
                cumulative_context,
                round_num,
            )
            .await?
        }
    };

    let resolved_count =
        mark_resolved_challenges(&mut challenges, &recovery_summary, adv.severity_threshold);
    run.metrics.challenges_resolved += resolved_count as u64;
    run.metrics.adversarial_rounds += 1;

    tracing::info!(
        round = round_num,
        resolved = resolved_count,
        actionable = actionable_count,
        mode = ?adv.recovery_mode,
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
        pre_eval_score: None,
        post_eval_score: None,
    })
}

// ── Recovery: Code-writing agents ──────────────────────────────────────────────

/// Spawn real code-writing agents to fix each actionable challenge sequentially.
///
/// Each agent gets the challenge description, suggested fix, and program.md
/// constraints. Agents run in the workspace with full coder tools and modify
/// files directly. Returns a summary compatible with `mark_resolved_challenges`.
async fn run_recovery_agents(
    ctx: &AdversarialContext<'_>,
    challenges: &[AdversarialChallenge],
    run: &mut SwarmRun,
    round: usize,
) -> Result<String, SwarmError> {
    let adv = &ctx.config.adversarial;
    let mut actionable: Vec<&AdversarialChallenge> = challenges
        .iter()
        .filter(|c| c.severity_score >= adv.severity_threshold)
        .collect();

    // Sort by severity descending, cap to max_recovery_agents.
    actionable.sort_by(|a, b| {
        b.severity_score
            .partial_cmp(&a.severity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    actionable.truncate(adv.max_recovery_agents);

    if actionable.is_empty() {
        return Ok(String::new());
    }

    tracing::info!(
        count = actionable.len(),
        mode = "fix",
        "spawning recovery agents"
    );

    let mut summaries = Vec::new();

    for challenge in &actionable {
        let summary = spawn_single_recovery_agent(ctx, challenge, run, round).await;
        summaries.push(summary);
    }

    Ok(summaries.join("\n"))
}

/// Spawn a single recovery agent for one challenge. Returns a `RESOLVED:` or `UNRESOLVED:` line.
#[allow(clippy::too_many_lines)]
async fn spawn_single_recovery_agent(
    ctx: &AdversarialContext<'_>,
    challenge: &AdversarialChallenge,
    run: &mut SwarmRun,
    round: usize,
) -> String {
    let adv = &ctx.config.adversarial;
    let fix_hint = challenge
        .suggested_fix
        .as_deref()
        .unwrap_or("no specific suggestion");

    let constraints = if ctx.program_md.is_empty() {
        String::new()
    } else {
        format!("## Constraints\n{}", ctx.program_md)
    };

    let system_prompt = format!(
        "You are a recovery agent fixing a specific issue found during adversarial review.\n\n\
         ## Issue to Fix\n\
         - **ID**: {id}\n- **Category**: {cat}\n- **Severity**: {severity:?} ({score:.2})\n\
         - **Description**: {desc}\n- **Suggested Fix**: {fix_hint}\n\n\
         ## Instructions\n\
         1. Read the relevant source files to understand the current state\n\
         2. Make the MINIMAL code changes needed to fix this specific issue\n\
         3. Run any relevant build/test commands to verify your fix\n\
         4. Do NOT modify unrelated code\n\
         5. Do NOT modify program.md, settings.json, or test infrastructure\n\n\
         {constraints}",
        id = challenge.id,
        cat = challenge.category,
        severity = challenge.severity,
        score = challenge.severity_score,
        desc = challenge.description,
    );

    let allowed_tools = vec![
        "Read".into(),
        "Write".into(),
        "Edit".into(),
        "Bash".into(),
        "Glob".into(),
        "Grep".into(),
    ];

    let subtask = SwarmSubtask {
        id: format!("recovery-{}", challenge.id),
        name: format!("Fix: {}", challenge.id),
        description: challenge.description.clone(),
        prompt: format!(
            "Fix this issue in the codebase: {}\n\nSuggested approach: {fix_hint}",
            challenge.description
        ),
        allowed_tools: Some(allowed_tools.clone()),
        timeout_seconds: adv.recovery_timeout_seconds,
    };

    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        role: "recovery".into(),
        task_id: format!("adversarial-round-{round}"),
        subtask_id: subtask.id.clone(),
        pid: None,
        workspace: ctx.working_dir.to_path_buf(),
        status: AgentSessionStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        actions_dispatched: 0,
        actions_blocked: 0,
    };

    tracing::info!(challenge_id = %challenge.id, severity = ?challenge.severity, "spawning recovery agent");
    run.metrics.agents_spawned += 1;

    let child = match spawn_agent(
        ctx.config,
        &session,
        &subtask,
        &system_prompt,
        &allowed_tools,
        ctx.hooks_binary,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(challenge_id = %challenge.id, "recovery agent spawn failed: {e}");
            run.metrics.agents_failed += 1;
            return format!("UNRESOLVED: {} - agent spawn failed", challenge.id);
        }
    };

    match wait_for_agent(child, &session.id, adv.recovery_timeout_seconds).await {
        Ok(result) if result.exit_code == 0 => {
            run.metrics.agents_completed += 1;
            tracing::info!(challenge_id = %challenge.id, "recovery agent completed");
            format!("RESOLVED: {} - agent applied fix", challenge.id)
        }
        Ok(result) => {
            run.metrics.agents_failed += 1;
            tracing::warn!(challenge_id = %challenge.id, exit_code = result.exit_code, "recovery agent failed");
            format!(
                "UNRESOLVED: {} - agent exited with code {}",
                challenge.id, result.exit_code
            )
        }
        Err(SwarmError::AgentTimeout { .. }) => {
            run.metrics.agents_failed += 1;
            tracing::warn!(challenge_id = %challenge.id, "recovery agent timed out");
            format!("UNRESOLVED: {} - agent timed out", challenge.id)
        }
        Err(e) => {
            run.metrics.agents_failed += 1;
            tracing::warn!(challenge_id = %challenge.id, "recovery agent error: {e}");
            format!("UNRESOLVED: {} - {e}", challenge.id)
        }
    }
}

// ── Recovery: Text-only analysis (legacy) ──────────────────────────────────────

/// Text-only recovery: describe fixes without editing code (original behavior).
async fn run_recovery_analyze(
    adv: &crate::config::AdversarialConfig,
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

    let truncated_output: String = primary_output.chars().take(3000).collect();

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

    invoke_engine(engine, &prompt, adv.recovery_timeout_seconds, "recovery").await
}

// ── Challenge phase ────────────────────────────────────────────────────────────

/// Run the challenge phase: spawn adversarial agents to critique the primary output.
async fn run_challenge_phase(
    adv: &crate::config::AdversarialConfig,
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

    let raw_output =
        invoke_engine(engine, &prompt, adv.challenge_timeout_seconds, "challenge").await?;
    Ok(parse_challenges(&raw_output, round))
}

// ── Engine invocation ──────────────────────────────────────────────────────────

/// Invoke an AI engine with a prompt and return the raw text output.
async fn invoke_engine(
    engine: AgentEngine,
    prompt: &str,
    timeout_seconds: u64,
    phase: &str,
) -> Result<String, SwarmError> {
    let cmd_name = match engine {
        AgentEngine::Claude => "claude",
        AgentEngine::Gemini => "gemini",
    };

    let mut cmd = tokio::process::Command::new(cmd_name);
    cmd.arg("-p").arg(prompt).arg("--output-format").arg("text");

    match engine {
        AgentEngine::Claude => {
            cmd.arg("--model").arg("haiku");
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

    let make_err = |msg: String| -> SwarmError {
        if phase == "recovery" {
            SwarmError::AdversarialRecovery(msg)
        } else {
            SwarmError::AdversarialChallenge(msg)
        }
    };

    match result {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(make_err(format!(
                "{cmd_name} exited with status {}: {stderr}",
                output.status
            )))
        }
        Ok(Err(e)) => Err(make_err(format!("failed to spawn {cmd_name}: {e}"))),
        Err(_) => Err(make_err(format!(
            "{cmd_name} timed out after {timeout_seconds}s"
        ))),
    }
}

// ── Parsing helpers ────────────────────────────────────────────────────────────

/// Parse challenge JSON lines from the adversarial agent's raw output.
fn parse_challenges(raw: &str, round: usize) -> Vec<AdversarialChallenge> {
    let mut challenges = Vec::new();
    let mut counter = 1;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = parse_single_challenge(line, round, &mut counter) {
            challenges.push(c);
        }
    }

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

fn parse_single_challenge(
    line: &str,
    round: usize,
    counter: &mut usize,
) -> Option<AdversarialChallenge> {
    let trimmed = line.strip_suffix(',').unwrap_or(line);
    let json = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    challenge_from_json(&json, round, counter)
}

fn challenge_from_json(
    json: &serde_json::Value,
    round: usize,
    counter: &mut usize,
) -> Option<AdversarialChallenge> {
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

        let recovery = "RESOLVED: c-1-1 \u{2014} Added null check to fix the bug\nUNRESOLVED: c-1-2 \u{2014} Style preference, not a blocker";

        let count = mark_resolved_challenges(&mut challenges, recovery, 0.5);
        assert_eq!(count, 1);
        assert!(challenges[0].resolved);
        assert_eq!(
            challenges[0].resolution.as_deref(),
            Some("Added null check to fix the bug")
        );
        assert!(!challenges[1].resolved);
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

    #[test]
    fn test_recovery_mode_serde() {
        let json_fix = serde_json::to_string(&RecoveryMode::Fix).unwrap();
        assert_eq!(json_fix, "\"fix\"");

        let json_analyze = serde_json::to_string(&RecoveryMode::Analyze).unwrap();
        assert_eq!(json_analyze, "\"analyze\"");

        let parsed: RecoveryMode = serde_json::from_str("\"fix\"").unwrap();
        assert_eq!(parsed, RecoveryMode::Fix);
    }
}
