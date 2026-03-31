use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::pin::Pin;

use chrono::Utc;
use futures::Future;
use futures::stream::{FuturesUnordered, StreamExt};
use uuid::Uuid;

use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::memory::TesseraiClient;
use crate::orchestrator::adversarial::run_adversarial_loop;
use crate::orchestrator::agent_spawner::{AgentResult, spawn_agent, wait_for_agent};
use crate::orchestrator::eval;
use crate::orchestrator::monitor::SwarmMonitor;
use crate::orchestrator::refiner::{apply_refinement, refine_plan};
use crate::roles::RoleRegistry;
use crate::types::adversarial::AdversarialResult;
use crate::types::agent::{AgentRole, AgentSession, AgentSessionStatus};
use crate::types::plan::{SwarmPlan, SwarmSubtask, SwarmTask};
use crate::types::run::{RunMetrics, SwarmRun, SwarmRunStatus, TaskRunStatus};

/// Owned context that can be cloned into `Send + 'static` task futures.
#[derive(Clone)]
struct SharedContext {
    config: SwarmConfig,
    roles: RoleRegistry,
    hooks_binary: PathBuf,
    run_id: String,
    tesserai: TesseraiClient,
}

/// Result of executing a single task (returned from the parallel future).
struct TaskResult {
    task_id: String,
    role_name: String,
    success: bool,
    subtask_outcomes: Vec<SubtaskOutcome>,
}

/// Outcome of a single subtask execution.
struct SubtaskOutcome {
    result: Option<AgentResult>,
}

/// Execute an approved swarm plan with parallel task execution.
///
/// Independent tasks (no dependency conflicts) run concurrently,
/// bounded by `max_agents`. After each task completes, the refiner
/// can add, skip, or reorder remaining tasks.
pub async fn execute_swarm(
    plan: &mut SwarmPlan,
    config: &SwarmConfig,
    roles: &RoleRegistry,
    hooks_binary: &std::path::Path,
) -> Result<SwarmRun, SwarmError> {
    let run_id = Uuid::new_v4().to_string();
    let tesserai = TesseraiClient::new(&config.tesserai)?;

    tracing::info!(run_id = %run_id, objective = %plan.objective, "starting swarm run");

    if let Err(e) = crate::memory::twins::create_run_twin(&tesserai, &run_id, plan).await {
        tracing::warn!("failed to create TesseraiDB run twin: {e}");
    }

    let mut run = SwarmRun {
        id: run_id.clone(),
        plan_id: plan.id.clone(),
        status: SwarmRunStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        task_status: plan
            .tasks
            .iter()
            .map(|t| (t.id.clone(), TaskRunStatus::Pending))
            .collect(),
        metrics: RunMetrics::default(),
    };

    let shared = SharedContext {
        config: config.clone(),
        roles: roles.clone(),
        hooks_binary: hooks_binary.to_path_buf(),
        run_id: run_id.clone(),
        tesserai: tesserai.clone(),
    };

    let mut monitor = SwarmMonitor::new();
    let mut completed_tasks: HashSet<String> = HashSet::new();
    let run_deadline = Utc::now()
        + chrono::Duration::minutes(
            i64::try_from(config.defaults.max_duration_minutes).unwrap_or(60),
        );

    run_orchestration_loop(
        plan,
        &shared,
        &mut run,
        &mut monitor,
        &mut completed_tasks,
        run_deadline,
    )
    .await?;

    finalize_run(&mut run, &tesserai, &run_id).await;
    crate::memory::graph::build_swarm_graph(&tesserai, &run_id, plan, &run).await;

    Ok(run)
}

/// Execute a swarm plan with an optional adversarial challenge-recovery loop.
///
/// If the adversarial config is enabled, the primary swarm runs first, then
/// an adversarial swarm challenges the output, and the primary engine recovers.
/// This can repeat for up to `max_rounds`.
///
/// Returns `(SwarmRun, Option<AdversarialResult>)`.
pub async fn execute_swarm_with_adversarial(
    plan: &mut SwarmPlan,
    config: &SwarmConfig,
    roles: &RoleRegistry,
    hooks_binary: &std::path::Path,
) -> Result<(SwarmRun, Option<AdversarialResult>), SwarmError> {
    let mut run = execute_swarm(plan, config, roles, hooks_binary).await?;

    let working_dir = config
        .defaults
        .working_directory
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Run baseline eval after primary swarm (if configured).
    let (eval_cfg, baseline_score) = if config.eval_harness.enabled {
        match eval::run_eval_harness(&config.eval_harness, &working_dir).await {
            Ok(result) => {
                let score = result.score;
                run.metrics.eval_baseline_score = Some(score);
                tracing::info!(score, passed = result.passed, "eval harness baseline");
                (Some(&config.eval_harness), Some(score))
            }
            Err(e) => {
                tracing::warn!("baseline eval failed: {e}");
                (Some(&config.eval_harness), None)
            }
        }
    } else {
        (None, None)
    };

    if !config.adversarial.enabled {
        return Ok((run, None));
    }

    // Collect primary output summary from completed task results.
    let primary_summary = build_primary_summary(plan, &run);

    let adversarial_result = run_adversarial_loop(
        config,
        plan,
        &mut run,
        &primary_summary,
        eval_cfg,
        baseline_score,
        &working_dir,
    )
    .await?;

    // Update final status based on adversarial outcome.
    if !adversarial_result.accepted && run.status == SwarmRunStatus::Completed {
        tracing::warn!(
            unresolved = adversarial_result.unresolved.len(),
            "adversarial review has unresolved challenges"
        );
    }

    // Persist adversarial report to the working directory.
    save_adversarial_report(config, &run.id, &adversarial_result);

    Ok((run, Some(adversarial_result)))
}

/// Build a summary of the primary swarm's output for the adversarial phase.
fn build_primary_summary(plan: &SwarmPlan, run: &SwarmRun) -> String {
    let mut sections = Vec::new();

    sections.push(format!("## Objective\n{}", plan.objective));

    sections.push(format!(
        "## Execution Status\n- Status: {:?}\n- Tasks: {}\n- Agents spawned: {}\n- Agents completed: {}\n- Agents failed: {}",
        run.status,
        run.task_status.len(),
        run.metrics.agents_spawned,
        run.metrics.agents_completed,
        run.metrics.agents_failed,
    ));

    let task_statuses: Vec<String> = plan
        .tasks
        .iter()
        .map(|t| {
            let status = run
                .task_status
                .get(&t.id)
                .map_or_else(|| "Unknown".into(), |s| format!("{s:?}"));
            format!("- {} ({}): {} — {status}", t.id, t.assigned_role, t.name)
        })
        .collect();

    sections.push(format!("## Task Results\n{}", task_statuses.join("\n")));

    sections.push(format!(
        "## Success Criteria\n{}",
        plan.success_criteria
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    ));

    sections.join("\n\n")
}

// ── Parallel orchestration loop ──────────────────────────────────────────────

/// Main loop: find ready tasks, run them concurrently, process completions.
async fn run_orchestration_loop(
    plan: &mut SwarmPlan,
    shared: &SharedContext,
    run: &mut SwarmRun,
    monitor: &mut SwarmMonitor,
    completed_tasks: &mut HashSet<String>,
    run_deadline: chrono::DateTime<Utc>,
) -> Result<(), SwarmError> {
    let mut in_flight: FuturesUnordered<Pin<Box<dyn Future<Output = TaskResult> + Send>>> =
        FuturesUnordered::new();
    let mut running_count: usize = 0;
    let mut role_counts: HashMap<String, usize> = HashMap::new();

    loop {
        if Utc::now() > run_deadline {
            tracing::warn!(run_id = %shared.run_id, "swarm run timed out");
            run.status = SwarmRunStatus::TimedOut;
            break;
        }

        // Find ready tasks: dependencies met, not already running/done, within concurrency limits.
        let max_agents = plan.scope.max_agents;
        let available_slots = max_agents.saturating_sub(running_count);

        let ready_tasks = find_ready_tasks(
            plan,
            completed_tasks,
            run,
            &role_counts,
            shared,
            available_slots,
        );

        // Spawn futures for each ready task.
        for (task, role) in &ready_tasks {
            run.task_status
                .insert(task.id.clone(), TaskRunStatus::Running);
            running_count += 1;
            *role_counts.entry(role.name.clone()).or_insert(0) += 1;

            run.metrics.agents_spawned += task.subtasks.len() as u64;

            tracing::info!(
                task_id = %task.id,
                role = %role.name,
                running = running_count,
                "starting task (parallel)"
            );

            let ctx = shared.clone();
            let task_owned = task.clone();
            let role_owned = role.clone();
            in_flight.push(Box::pin(async move {
                execute_task_isolated(ctx, task_owned, role_owned).await
            }));
        }

        // If nothing in flight and nothing spawned, we're done.
        if in_flight.is_empty() {
            break;
        }

        // Wait for the next task to complete.
        if let Some(result) = in_flight.next().await {
            running_count -= 1;
            if let Some(count) = role_counts.get_mut(&result.role_name) {
                *count = count.saturating_sub(1);
            }

            handle_task_completion(plan, shared, run, monitor, completed_tasks, result).await?;
        }
    }

    Ok(())
}

/// Find tasks whose dependencies are met, sorted by priority, respecting concurrency limits.
fn find_ready_tasks(
    plan: &SwarmPlan,
    completed_tasks: &HashSet<String>,
    run: &SwarmRun,
    role_counts: &HashMap<String, usize>,
    shared: &SharedContext,
    available_slots: usize,
) -> Vec<(SwarmTask, AgentRole)> {
    if available_slots == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<&SwarmTask> = plan
        .tasks
        .iter()
        .filter(|t| {
            !completed_tasks.contains(&t.id)
                && !matches!(
                    run.task_status.get(&t.id),
                    Some(
                        TaskRunStatus::Running
                            | TaskRunStatus::Completed
                            | TaskRunStatus::Failed(_)
                            | TaskRunStatus::Skipped
                    )
                )
                && t.depends_on.iter().all(|dep| completed_tasks.contains(dep))
        })
        .collect();

    // Sort by priority (lower number = higher priority).
    candidates.sort_by_key(|t| t.priority);

    let mut result = Vec::new();
    for task in candidates {
        if result.len() >= available_slots {
            break;
        }

        let Some(role) = shared.roles.get(&task.assigned_role) else {
            continue;
        };

        // Enforce per-role concurrency limit.
        let current_role_count = role_counts.get(&task.assigned_role).copied().unwrap_or(0);
        if current_role_count >= role.max_concurrent_instances {
            continue;
        }

        result.push((task.clone(), role.clone()));
    }

    result
}

// ── Isolated task execution (runs in parallel future) ────────────────────────

/// Execute a task and all its subtasks. Pure async — no mutable shared state.
/// Returns a `TaskResult` for the driver to process.
async fn execute_task_isolated(ctx: SharedContext, task: SwarmTask, role: AgentRole) -> TaskResult {
    let mut outcomes = Vec::new();

    for subtask in &task.subtasks {
        let session = build_agent_session_owned(&ctx, &task.id, subtask, &role);

        // Create session twin.
        if let Err(e) =
            crate::memory::twins::create_session_twin(&ctx.tesserai, &ctx.run_id, &session).await
        {
            tracing::warn!("failed to create session twin: {e}");
        }

        // Retrieve prior findings.
        let prior_findings =
            crate::memory::semantic::retrieve_prior_context(&ctx.tesserai, 5, 2000).await;
        let prior_context = crate::memory::semantic::format_prior_context(&prior_findings);

        let base_prompt = crate::roles::prompt_builder::build_system_prompt(&role, &task, subtask);
        let system_prompt = if prior_context.is_empty() {
            base_prompt
        } else {
            tracing::info!(
                count = prior_findings.len(),
                task = %task.id,
                "injecting prior findings"
            );
            format!("{base_prompt}{prior_context}")
        };

        let allowed_tools = subtask
            .allowed_tools
            .as_ref()
            .unwrap_or(&role.allowed_tools);

        // Spawn and wait.
        let child = match spawn_agent(
            &ctx.config,
            &session,
            subtask,
            &system_prompt,
            allowed_tools,
            &ctx.hooks_binary,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(subtask = %subtask.id, error = %e, "agent spawn failed");
                outcomes.push(SubtaskOutcome { result: None });
                return TaskResult {
                    task_id: task.id.clone(),
                    role_name: role.name.clone(),
                    success: false,
                    subtask_outcomes: outcomes,
                };
            }
        };

        let timeout = subtask
            .timeout_seconds
            .max(ctx.config.defaults.subtask_timeout_seconds);

        match wait_for_agent(child, &session.id, timeout).await {
            Ok(result) => {
                // Store memories immediately (from the future, using owned ctx).
                store_memories_isolated(&ctx, &session, &task, subtask, &role, &result).await;

                let failed = result.exit_code != 0;
                outcomes.push(SubtaskOutcome {
                    result: Some(result),
                });

                if failed {
                    return TaskResult {
                        task_id: task.id.clone(),
                        role_name: role.name.clone(),
                        success: false,
                        subtask_outcomes: outcomes,
                    };
                }
            }
            Err(SwarmError::AgentTimeout { .. }) => {
                tracing::warn!(subtask = %subtask.id, "agent timed out");
                outcomes.push(SubtaskOutcome { result: None });
                return TaskResult {
                    task_id: task.id.clone(),
                    role_name: role.name.clone(),
                    success: false,
                    subtask_outcomes: outcomes,
                };
            }
            Err(e) => {
                tracing::error!(subtask = %subtask.id, error = %e, "agent failed");
                outcomes.push(SubtaskOutcome { result: None });
                return TaskResult {
                    task_id: task.id.clone(),
                    role_name: role.name.clone(),
                    success: false,
                    subtask_outcomes: outcomes,
                };
            }
        }
    }

    TaskResult {
        task_id: task.id.clone(),
        role_name: role.name.clone(),
        success: true,
        subtask_outcomes: outcomes,
    }
}

/// Store episodic + semantic memories from inside the isolated future.
async fn store_memories_isolated(
    ctx: &SharedContext,
    session: &AgentSession,
    task: &SwarmTask,
    subtask: &SwarmSubtask,
    role: &AgentRole,
    result: &AgentResult,
) {
    let summary = extract_result_text(&result.result_text);

    let _ = crate::memory::semantic::record_action(
        &ctx.tesserai,
        &ctx.run_id,
        &session.id,
        &role.name,
        &summary,
        vec![task.name.clone(), subtask.name.clone(), role.name.clone()],
        None,
    )
    .await;

    if summary.len() > 100 {
        let _ = crate::memory::semantic::store_finding(
            &ctx.tesserai,
            &ctx.run_id,
            &session.id,
            &summary,
            vec![task.name.clone(), role.name.clone()],
            0.8,
        )
        .await;
    }
}

// ── Completion handling (runs in the driver, has mutable access) ──────────────

/// Process a completed task: update metrics, run refiner, mark completed.
async fn handle_task_completion(
    plan: &mut SwarmPlan,
    shared: &SharedContext,
    run: &mut SwarmRun,
    monitor: &mut SwarmMonitor,
    completed_tasks: &mut HashSet<String>,
    result: TaskResult,
) -> Result<(), SwarmError> {
    // Update metrics from subtask outcomes.
    for outcome in &result.subtask_outcomes {
        if let Some(ref agent_result) = outcome.result {
            if agent_result.exit_code == 0 {
                run.metrics.agents_completed += 1;
            } else {
                run.metrics.agents_failed += 1;
            }
            run.metrics.total_actions += 1;
            run.metrics.memories_stored += 1; // episodic
            if extract_result_text(&agent_result.result_text).len() > 100 {
                run.metrics.memories_stored += 1; // semantic finding
            }
        } else {
            run.metrics.agents_failed += 1;
            run.metrics.total_actions += 1;
        }
    }

    // Update task status.
    if result.success {
        run.task_status
            .insert(result.task_id.clone(), TaskRunStatus::Completed);
        completed_tasks.insert(result.task_id.clone());
        tracing::info!(task_id = %result.task_id, "task completed");
    } else {
        run.task_status.insert(
            result.task_id.clone(),
            TaskRunStatus::Failed("subtask failed".into()),
        );
        tracing::warn!(task_id = %result.task_id, "task failed");
    }

    monitor.remove_agent(&result.task_id);

    // Run refiner for the last subtask with output.
    if let Some(output) = shared
        .config
        .defaults
        .enable_refiner
        .then(|| {
            result
                .subtask_outcomes
                .iter()
                .rev()
                .find_map(|o| o.result.as_ref())
        })
        .flatten()
    {
        let completed_refs: Vec<&str> = completed_tasks.iter().map(String::as_str).collect();
        match refine_plan(
            &shared.config,
            &shared.roles,
            plan,
            &result.task_id,
            &output.result_text,
            &completed_refs,
        )
        .await
        {
            Ok(action) => {
                let skipped = apply_refinement(plan, &action);
                // Mark skipped tasks as completed so dependents can proceed.
                for id in &skipped {
                    run.task_status.insert(id.clone(), TaskRunStatus::Skipped);
                    completed_tasks.insert(id.clone());
                }
                if !matches!(
                    action,
                    crate::orchestrator::refiner::RefinementAction::Continue
                ) {
                    run.metrics.refinements += 1;
                    tracing::info!(action = ?action, "refiner adjusted plan");
                }
            }
            Err(e) => tracing::warn!("refiner failed: {e}"),
        }
    }

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build an `AgentSession` from owned context.
fn build_agent_session_owned(
    ctx: &SharedContext,
    task_id: &str,
    subtask: &SwarmSubtask,
    role: &AgentRole,
) -> AgentSession {
    AgentSession {
        id: Uuid::new_v4().to_string(),
        role: role.name.clone(),
        task_id: task_id.to_string(),
        subtask_id: subtask.id.clone(),
        pid: None,
        workspace: ctx
            .config
            .defaults
            .working_directory
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
        status: AgentSessionStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        actions_dispatched: 0,
        actions_blocked: 0,
    }
}

/// Determine final run status and update the `TesseraiDB` twin.
async fn finalize_run(run: &mut SwarmRun, tesserai: &TesseraiClient, run_id: &str) {
    let all_completed = run
        .task_status
        .values()
        .all(|s| matches!(s, TaskRunStatus::Completed | TaskRunStatus::Skipped));

    run.status = if all_completed {
        SwarmRunStatus::Completed
    } else if run.status == SwarmRunStatus::TimedOut {
        SwarmRunStatus::TimedOut
    } else {
        SwarmRunStatus::Failed
    };

    run.finished_at = Some(Utc::now());

    let status_str = match run.status {
        SwarmRunStatus::Completed => "completed",
        SwarmRunStatus::Failed => "failed",
        SwarmRunStatus::TimedOut => "timed_out",
        SwarmRunStatus::Cancelled => "cancelled",
        SwarmRunStatus::Adversarial => "adversarial",
        _ => "unknown",
    };
    if let Err(e) = crate::memory::twins::update_run_status(tesserai, run_id, status_str).await {
        tracing::warn!("failed to update run twin: {e}");
    }

    tracing::info!(
        run_id = %run_id,
        status = ?run.status,
        agents_spawned = run.metrics.agents_spawned,
        agents_completed = run.metrics.agents_completed,
        "swarm run finished"
    );
}

/// Save the adversarial report as JSON in the working directory.
///
/// Writes to `<working_dir>/adversarial-report-<run_id>.json`.
fn save_adversarial_report(config: &SwarmConfig, run_id: &str, result: &AdversarialResult) {
    let dir = config
        .defaults
        .working_directory
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let filename = format!("adversarial-report-{run_id}.json");
    let path = dir.join(&filename);

    match serde_json::to_string_pretty(result) {
        Ok(json) => match std::fs::write(&path, &json) {
            Ok(()) => tracing::info!(path = %path.display(), "adversarial report saved"),
            Err(e) => {
                tracing::warn!(path = %path.display(), "failed to save adversarial report: {e}");
            }
        },
        Err(e) => tracing::warn!("failed to serialize adversarial report: {e}"),
    }
}

/// Extract the readable result text from agent output.
///
/// Handles both formats:
/// - `claude -p --output-format json`: single JSON blob with `.result` field
/// - Agent SDK bridge (NDJSON): multiple lines, look for `{"type":"result","content":"..."}`
///   and `{"type":"text","content":"..."}` lines
fn extract_result_text(raw: &str) -> String {
    // Try single JSON blob first (claude -p format).
    if let Some(result) = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|json| {
            json.get("result")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
    {
        return result;
    }

    // Try NDJSON (Agent SDK bridge format): collect text blocks, then check result line.
    let mut texts = Vec::new();
    let mut final_result = String::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
            match json.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(content) = json.get("content").and_then(|v| v.as_str()) {
                        texts.push(content.to_string());
                    }
                }
                Some("result") => {
                    if let Some(content) = json
                        .get("content")
                        .and_then(|v| v.as_str())
                        .filter(|c| !c.is_empty())
                    {
                        final_result = content.to_string();
                    }
                }
                _ => {}
            }
        }
    }

    if !final_result.is_empty() {
        return final_result;
    }
    if !texts.is_empty() {
        return texts.join("\n");
    }

    // Fallback: raw text, truncated.
    raw.chars().take(2000).collect()
}
