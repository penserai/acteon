use std::collections::HashSet;

use chrono::Utc;
use uuid::Uuid;

use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::memory::TesseraiClient;
use crate::orchestrator::agent_spawner::{AgentResult, spawn_agent, wait_for_agent};
use crate::orchestrator::monitor::SwarmMonitor;
use crate::orchestrator::refiner::{apply_refinement, refine_plan};
use crate::roles::RoleRegistry;
use crate::types::agent::{AgentSession, AgentSessionStatus};
use crate::types::plan::SwarmPlan;
use crate::types::run::{RunMetrics, SwarmRun, SwarmRunStatus, TaskRunStatus};

/// Shared context for orchestration, grouping immutable references.
struct SwarmContext<'a> {
    config: &'a SwarmConfig,
    roles: &'a RoleRegistry,
    hooks_binary: &'a std::path::Path,
    run_id: &'a str,
    tesserai: &'a TesseraiClient,
}

/// Execute an approved swarm plan.
///
/// This is the main orchestration loop that:
/// 1. Sets up Acteon quotas and safety rules
/// 2. Creates `TesseraiDB` twins for tracking
/// 3. Spawns agents for each subtask in dependency order
/// 4. Monitors execution and handles completion/failure
/// 5. Runs the refiner after each subtask
/// 6. Cleans up resources
pub async fn execute_swarm(
    plan: &mut SwarmPlan,
    config: &SwarmConfig,
    roles: &RoleRegistry,
    hooks_binary: &std::path::Path,
) -> Result<SwarmRun, SwarmError> {
    let run_id = Uuid::new_v4().to_string();
    let tesserai = TesseraiClient::new(&config.tesserai)?;

    tracing::info!(run_id = %run_id, objective = %plan.objective, "starting swarm run");

    // Create TesseraiDB twin for the run.
    if let Err(e) = crate::memory::twins::create_run_twin(&tesserai, &run_id, plan).await {
        tracing::warn!("failed to create TesseraiDB run twin: {e}");
    }

    // TODO (Phase 3): Create Acteon quota and deploy safety rules.

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

    let ctx = SwarmContext {
        config,
        roles,
        hooks_binary,
        run_id: &run_id,
        tesserai: &tesserai,
    };

    let mut monitor = SwarmMonitor::new();
    let mut completed_tasks: HashSet<String> = HashSet::new();
    let run_deadline = Utc::now()
        + chrono::Duration::minutes(
            i64::try_from(config.defaults.max_duration_minutes).unwrap_or(60),
        );

    run_orchestration_loop(
        plan,
        &ctx,
        &mut run,
        &mut monitor,
        &mut completed_tasks,
        run_deadline,
    )
    .await?;

    finalize_run(&mut run, &tesserai, &run_id).await;

    Ok(run)
}

/// Main orchestration loop: schedule and execute tasks until completion or timeout.
async fn run_orchestration_loop(
    plan: &mut SwarmPlan,
    ctx: &SwarmContext<'_>,
    run: &mut SwarmRun,
    monitor: &mut SwarmMonitor,
    completed_tasks: &mut HashSet<String>,
    run_deadline: chrono::DateTime<Utc>,
) -> Result<(), SwarmError> {
    loop {
        // Check timeout.
        if Utc::now() > run_deadline {
            tracing::warn!(run_id = %ctx.run_id, "swarm run timed out");
            run.status = SwarmRunStatus::TimedOut;
            break;
        }

        // Find tasks whose dependencies are all complete.
        let ready_tasks: Vec<String> = plan
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
            .map(|t| t.id.clone())
            .collect();

        if ready_tasks.is_empty()
            && !run
                .task_status
                .values()
                .any(|s| matches!(s, TaskRunStatus::Running))
        {
            // Nothing running and nothing ready — we're done.
            break;
        }

        // Execute ready tasks (sequentially for now; Phase 6 will add concurrency).
        for task_id in ready_tasks {
            execute_single_task(plan, ctx, run, monitor, completed_tasks, &task_id).await?;
        }

        // Only exit if no tasks are running AND no tasks are ready to start.
        let any_running = run
            .task_status
            .values()
            .any(|s| matches!(s, TaskRunStatus::Running));
        let any_newly_ready = plan.tasks.iter().any(|t| {
            !completed_tasks.contains(&t.id)
                && matches!(
                    run.task_status.get(&t.id),
                    Some(TaskRunStatus::Pending) | None
                )
                && t.depends_on
                    .iter()
                    .all(|dep| completed_tasks.contains(dep))
        });
        if !any_running && !any_newly_ready {
            break;
        }
    }

    Ok(())
}

/// Execute a single task and all its subtasks, updating run state accordingly.
async fn execute_single_task(
    plan: &mut SwarmPlan,
    ctx: &SwarmContext<'_>,
    run: &mut SwarmRun,
    monitor: &mut SwarmMonitor,
    completed_tasks: &mut HashSet<String>,
    task_id: &str,
) -> Result<(), SwarmError> {
    // Clone task data to avoid borrow conflicts with plan mutation in refiner.
    let task = plan.tasks.iter().find(|t| t.id == task_id).unwrap().clone();

    let role = ctx
        .roles
        .get(&task.assigned_role)
        .ok_or_else(|| SwarmError::UnknownRole(task.assigned_role.clone()))?;

    run.task_status
        .insert(task_id.to_string(), TaskRunStatus::Running);
    tracing::info!(task_id = %task_id, role = %role.name, "starting task");

    let task_failed =
        execute_subtasks(plan, ctx, run, completed_tasks, task_id, &task, role).await?;

    if task_failed {
        run.task_status.insert(
            task_id.to_string(),
            TaskRunStatus::Failed("subtask failed".into()),
        );
    } else {
        run.task_status
            .insert(task_id.to_string(), TaskRunStatus::Completed);
        completed_tasks.insert(task_id.to_string());
    }

    monitor.remove_agent(task_id);

    Ok(())
}

/// Execute all subtasks for a task. Returns `true` if any subtask failed.
async fn execute_subtasks(
    plan: &mut SwarmPlan,
    ctx: &SwarmContext<'_>,
    run: &mut SwarmRun,
    completed_tasks: &HashSet<String>,
    task_id: &str,
    task: &crate::types::plan::SwarmTask,
    role: &crate::types::agent::AgentRole,
) -> Result<bool, SwarmError> {
    for subtask in &task.subtasks {
        let session = build_agent_session(ctx, task_id, subtask, role);

        // Create TesseraiDB twin for the session.
        if let Err(e) =
            crate::memory::twins::create_session_twin(ctx.tesserai, ctx.run_id, &session).await
        {
            tracing::warn!("failed to create session twin: {e}");
        }

        run.metrics.agents_spawned += 1;

        // Retrieve prior findings from TesseraiDB and inject as context.
        let prior_findings =
            crate::memory::semantic::retrieve_prior_context(ctx.tesserai, 5, 2000).await;
        let prior_context = crate::memory::semantic::format_prior_context(&prior_findings);

        let base_prompt = crate::roles::prompt_builder::build_system_prompt(role, task, subtask);
        let system_prompt = if prior_context.is_empty() {
            base_prompt
        } else {
            tracing::info!(
                count = prior_findings.len(),
                "injecting prior findings into agent prompt"
            );
            format!("{base_prompt}{prior_context}")
        };

        let allowed_tools = subtask
            .allowed_tools
            .as_ref()
            .unwrap_or(&role.allowed_tools);

        // Spawn the agent.
        let child: tokio::process::Child = spawn_agent(
            ctx.config,
            &session,
            subtask,
            &system_prompt,
            allowed_tools,
            ctx.hooks_binary,
        )
        .await?;

        // Wait for completion. Use the larger of plan timeout vs config default.
        let timeout = subtask
            .timeout_seconds
            .max(ctx.config.defaults.subtask_timeout_seconds);
        match wait_for_agent(child, &session.id, timeout).await {
            Ok(result) => {
                update_agent_metrics(&result, run);
                store_agent_memories(ctx, run, &session, task, subtask, role, &result).await;

                // Run refiner.
                let completed_refs: Vec<&str> =
                    completed_tasks.iter().map(String::as_str).collect();
                match refine_plan(
                    ctx.config,
                    plan,
                    task_id,
                    &result.result_text,
                    &completed_refs,
                )
                .await
                {
                    Ok(action) => {
                        apply_refinement(plan, &action);
                        if !matches!(
                            action,
                            crate::orchestrator::refiner::RefinementAction::Continue
                        ) {
                            run.metrics.refinements += 1;
                        }
                    }
                    Err(e) => tracing::warn!("refiner failed: {e}"),
                }
            }
            Err(SwarmError::AgentTimeout { .. }) => {
                tracing::warn!(subtask = %subtask.id, "agent timed out");
                run.metrics.agents_failed += 1;
                return Ok(true);
            }
            Err(e) => {
                tracing::error!(subtask = %subtask.id, error = %e, "agent failed");
                run.metrics.agents_failed += 1;
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Build an `AgentSession` for a subtask.
fn build_agent_session(
    ctx: &SwarmContext<'_>,
    task_id: &str,
    subtask: &crate::types::plan::SwarmSubtask,
    role: &crate::types::agent::AgentRole,
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

    // Update TesseraiDB run twin.
    let status_str = match run.status {
        SwarmRunStatus::Completed => "completed",
        SwarmRunStatus::Failed => "failed",
        SwarmRunStatus::TimedOut => "timed_out",
        SwarmRunStatus::Cancelled => "cancelled",
        _ => "unknown",
    };
    if let Err(e) = crate::memory::twins::update_run_status(tesserai, run_id, status_str).await {
        tracing::warn!("failed to update run twin: {e}");
    }

    tracing::info!(
        run_id = %run_id,
        status = ?run.status,
        agents_spawned = run.metrics.agents_spawned,
        "swarm run finished"
    );
}

/// Store episodic and semantic memories in `TesseraiDB` after an agent completes.
async fn store_agent_memories(
    ctx: &SwarmContext<'_>,
    run: &mut SwarmRun,
    session: &AgentSession,
    task: &crate::types::plan::SwarmTask,
    subtask: &crate::types::plan::SwarmSubtask,
    role: &crate::types::agent::AgentRole,
    result: &AgentResult,
) {
    let result_summary = extract_result_text(&result.result_text);

    // Store episodic memory of what the agent did.
    if let Err(e) = crate::memory::semantic::record_action(
        ctx.tesserai,
        ctx.run_id,
        &session.id,
        &role.name,
        &result_summary,
        vec![
            task.name.clone(),
            subtask.name.clone(),
            role.name.clone(),
        ],
        None,
    )
    .await
    {
        tracing::debug!("failed to store episodic memory: {e}");
    } else {
        run.metrics.memories_stored += 1;
    }

    // Store as a semantic finding if it produced meaningful content.
    if result_summary.len() > 100 {
        if let Err(e) = crate::memory::semantic::store_finding(
            ctx.tesserai,
            ctx.run_id,
            &session.id,
            &result_summary,
            vec![task.name.clone(), role.name.clone()],
            0.8,
        )
        .await
        {
            tracing::debug!("failed to store finding: {e}");
        } else {
            run.metrics.memories_stored += 1;
        }
    }
}

fn update_agent_metrics(result: &AgentResult, run: &mut SwarmRun) {
    if result.exit_code == 0 {
        run.metrics.agents_completed += 1;
    } else {
        run.metrics.agents_failed += 1;
    }
    run.metrics.total_actions += 1;
}

/// Extract the readable result text from claude's JSON output.
fn extract_result_text(raw: &str) -> String {
    // claude -p --output-format json wraps result in {"result": "...", ...}
    if let Some(result) = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|json| json.get("result").and_then(|v| v.as_str()).map(String::from))
    {
        return result;
    }
    // Fallback: use raw text, truncated.
    raw.chars().take(2000).collect()
}
