use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use tracing::{info, warn};

use acteon_swarm::config::SwarmConfig;
use acteon_swarm::planner::gatherer::gather_plan;
use acteon_swarm::planner::validate_plan;
use acteon_swarm::roles::RoleRegistry;
use acteon_swarm::types::plan::SwarmPlan;

#[derive(Parser)]
#[command(
    name = "acteon-swarm",
    version,
    about = "Orchestrate multi-agent swarms via Acteon + TesseraiDB + AI Agents"
)]
struct Cli {
    /// Path to swarm configuration file.
    #[arg(short, long, default_value = "swarm.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Plan management (gather, show, approve).
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Execute an approved plan.
    Run(RunArgs),
    /// Show the status of a running or completed swarm.
    Status(StatusArgs),
    /// Cancel a running swarm.
    Cancel(CancelArgs),
}

#[derive(Subcommand)]
enum PlanAction {
    /// Gather a new plan via interactive Q&A with the AI engine.
    Gather {
        /// The high-level objective or prompt.
        #[arg(short, long)]
        prompt: String,
        /// Output file for the plan (default: plan.json).
        #[arg(short, long, default_value = "plan.json")]
        output: PathBuf,
    },
    /// Display a plan file in human-readable format.
    Show {
        /// Path to the plan file.
        #[arg(short, long, default_value = "plan.json")]
        plan: PathBuf,
    },
    /// Mark a plan as approved.
    Approve {
        /// Path to the plan file.
        #[arg(short, long, default_value = "plan.json")]
        plan: PathBuf,
    },
}

#[derive(Parser)]
struct RunArgs {
    /// Path to an approved plan file.
    #[arg(short, long)]
    plan: Option<PathBuf>,
    /// Or: gather and run in one step.
    #[arg(long)]
    prompt: Option<String>,
    /// Skip plan approval (use with --prompt).
    #[arg(long, default_value_t = false)]
    auto_approve: bool,
    /// Enable adversarial challenge-recovery loop after the primary swarm.
    #[arg(long, default_value_t = false)]
    adversarial: bool,
    /// AI engine for the adversarial swarm (overrides config; e.g., "claude" or "gemini").
    #[arg(long)]
    adversarial_engine: Option<String>,
    /// Maximum adversarial challenge-recovery rounds (overrides config).
    #[arg(long)]
    adversarial_rounds: Option<usize>,
    /// Eval harness command (e.g., "cargo test && cargo clippy").
    #[arg(long)]
    eval_command: Option<String>,
    /// Eval harness timeout in seconds (overrides config).
    #[arg(long)]
    eval_timeout: Option<u64>,
    /// Minimum eval score to consider passing (0.0-1.0, overrides config).
    #[arg(long)]
    eval_threshold: Option<f64>,
    /// Recovery mode: "fix" (spawn code-writing agents) or "analyze" (text-only).
    #[arg(long)]
    recovery_mode: Option<String>,
}

#[derive(Parser)]
struct StatusArgs {
    /// Swarm run ID.
    #[arg(short, long)]
    run: String,
}

#[derive(Parser)]
struct CancelArgs {
    /// Swarm run ID.
    #[arg(short, long)]
    run: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "acteon_swarm=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    match cli.command {
        Commands::Plan { action } => match action {
            PlanAction::Gather { prompt, output } => {
                cmd_plan_gather(&config, &prompt, &output).await?;
            }
            PlanAction::Show { plan } => {
                cmd_plan_show(&plan)?;
            }
            PlanAction::Approve { plan } => {
                cmd_plan_approve(&plan)?;
            }
        },
        Commands::Run(args) => {
            cmd_run(&config, args).await?;
        }
        Commands::Status(args) => {
            cmd_status(&config, &args.run).await?;
        }
        Commands::Cancel(args) => {
            cmd_cancel(&config, &args.run);
        }
    }

    Ok(())
}

fn load_config(path: &std::path::Path) -> Result<SwarmConfig> {
    if path.exists() {
        Ok(SwarmConfig::from_file(path)?)
    } else {
        info!("no config file found at {}, using defaults", path.display());
        Ok(SwarmConfig::minimal())
    }
}

async fn cmd_plan_gather(config: &SwarmConfig, prompt: &str, output: &PathBuf) -> Result<()> {
    let engine = config.defaults.engine;
    info!(engine = ?engine, "gathering plan");
    let plan = gather_plan(config, prompt).await?;

    let roles = RoleRegistry::with_config(engine, &config.roles);
    let warnings = validate_plan(&plan, &roles.names())?;

    for w in &warnings {
        warn!(message = %w.message, "plan validation warning");
    }

    let json = serde_json::to_string_pretty(&plan)?;
    std::fs::write(output, &json)?;
    info!(
        path = %output.display(),
        tasks = plan.tasks.len(),
        estimated_actions = plan.estimated_actions,
        "plan saved"
    );
    info!("review with: acteon-swarm plan show");
    info!("approve with: acteon-swarm plan approve");

    Ok(())
}

fn cmd_plan_show(path: &PathBuf) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let plan: SwarmPlan = serde_json::from_str(&content)?;

    info!(
        objective = %plan.objective,
        id = %plan.id,
        status = if plan.is_approved() { "APPROVED" } else { "PENDING" },
        estimated_actions = plan.estimated_actions,
        tasks = plan.tasks.len(),
        "plan summary"
    );

    for c in &plan.success_criteria {
        info!(criterion = %c, "success criterion");
    }

    for task in &plan.tasks {
        let deps = if task.depends_on.is_empty() {
            String::new()
        } else {
            format!(" (depends: {})", task.depends_on.join(", "))
        };
        info!(
            task_id = %task.id,
            name = %task.name,
            role = %task.assigned_role,
            subtasks = task.subtasks.len(),
            "task{deps}"
        );
    }

    Ok(())
}

fn cmd_plan_approve(path: &PathBuf) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut plan: SwarmPlan = serde_json::from_str(&content)?;

    if plan.is_approved() {
        info!("plan is already approved");
        return Ok(());
    }

    plan.approved_at = Some(Utc::now());
    let json = serde_json::to_string_pretty(&plan)?;
    std::fs::write(path, json)?;
    info!(approved_at = %plan.approved_at.unwrap(), "plan approved");

    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn cmd_run(config: &SwarmConfig, args: RunArgs) -> Result<()> {
    let mut plan = if let Some(ref plan_path) = args.plan {
        let content = std::fs::read_to_string(plan_path)?;
        serde_json::from_str(&content)?
    } else if let Some(ref prompt) = args.prompt {
        let plan = gather_plan(config, prompt).await?;
        if args.auto_approve {
            let mut p = plan;
            p.approved_at = Some(Utc::now());
            p
        } else {
            info!("plan gathered — approve with: acteon-swarm plan approve");
            let json = serde_json::to_string_pretty(&plan)?;
            std::fs::write("plan.json", json)?;
            return Ok(());
        }
    } else {
        anyhow::bail!("provide --plan or --prompt");
    };

    if !plan.is_approved() {
        anyhow::bail!("plan is not approved. Run: acteon-swarm plan approve");
    }

    // Apply CLI overrides to adversarial config.
    let mut config = config.clone();
    if args.adversarial {
        config.adversarial.enabled = true;
    }
    if let Some(ref engine_str) = args.adversarial_engine {
        config.adversarial.enabled = true;
        config.adversarial.engine = Some(parse_engine(engine_str)?);
    }
    if let Some(rounds) = args.adversarial_rounds {
        config.adversarial.max_rounds = rounds;
    }
    if let Some(ref cmd) = args.eval_command {
        config.eval_harness.enabled = true;
        config.eval_harness.command.clone_from(cmd);
    }
    if let Some(timeout) = args.eval_timeout {
        config.eval_harness.timeout_seconds = timeout;
    }
    if let Some(threshold) = args.eval_threshold {
        config.eval_harness.pass_threshold = threshold;
    }
    if let Some(ref mode) = args.recovery_mode {
        config.adversarial.recovery_mode = match mode.to_lowercase().as_str() {
            "fix" => acteon_swarm::config::RecoveryMode::Fix,
            "analyze" => acteon_swarm::config::RecoveryMode::Analyze,
            other => anyhow::bail!("unknown recovery mode: {other} (expected 'fix' or 'analyze')"),
        };
    }

    let roles = RoleRegistry::with_config(config.defaults.engine, &config.roles);
    validate_plan(&plan, &roles.names())?;

    // Find the hook binary (should be next to this binary).
    let hooks_binary = std::env::current_exe()?
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("acteon-swarm-hook");

    info!("starting swarm run");

    if config.adversarial.enabled {
        let adv_engine = config.adversarial.effective_engine(config.defaults.engine);
        info!(
            engine = ?adv_engine,
            max_rounds = config.adversarial.max_rounds,
            "adversarial mode enabled"
        );
    }

    let (run, adversarial_result) =
        acteon_swarm::execute_swarm_with_adversarial(&mut plan, &config, &roles, &hooks_binary)
            .await?;

    info!(
        status = ?run.status,
        agents_spawned = run.metrics.agents_spawned,
        agents_completed = run.metrics.agents_completed,
        agents_failed = run.metrics.agents_failed,
        total_actions = run.metrics.total_actions,
        refinements = run.metrics.refinements,
        eval_baseline = ?run.metrics.eval_baseline_score,
        eval_final = ?run.metrics.eval_final_score,
        "swarm run complete"
    );

    if let Some(adv) = adversarial_result {
        info!(
            rounds = adv.rounds.len(),
            total_challenges = adv.total_challenges,
            resolved = adv.total_resolved,
            accepted = adv.accepted,
            unresolved = adv.unresolved.len(),
            "adversarial review"
        );

        for c in &adv.unresolved {
            warn!(
                id = %c.id,
                severity = ?c.severity,
                category = %c.category,
                description = %c.description,
                "unresolved challenge"
            );
        }

        let report_dir = config
            .defaults
            .working_directory
            .as_deref()
            .unwrap_or(std::path::Path::new("."));
        info!(
            path = %format!("{}/adversarial-report-{}.json", report_dir.display(), run.id),
            "adversarial report saved"
        );
    }

    Ok(())
}

fn parse_engine(s: &str) -> Result<acteon_swarm::config::AgentEngine> {
    match s.to_lowercase().as_str() {
        "claude" => Ok(acteon_swarm::config::AgentEngine::Claude),
        "gemini" => Ok(acteon_swarm::config::AgentEngine::Gemini),
        other => anyhow::bail!("unknown engine: {other} (expected 'claude' or 'gemini')"),
    }
}

async fn cmd_status(config: &SwarmConfig, run_id: &str) -> Result<()> {
    let summary = acteon_swarm::acteon::audit_watcher::fetch_audit_summary(config, run_id).await?;

    info!(
        run_id = %run_id,
        total_dispatched = summary.total_dispatched,
        executed = summary.executed,
        suppressed = summary.suppressed,
        throttled = summary.throttled,
        deduplicated = summary.deduplicated,
        pending_approval = summary.pending_approval,
        quota_exceeded = summary.quota_exceeded,
        rerouted = summary.rerouted,
        "swarm run status"
    );

    Ok(())
}

fn cmd_cancel(_config: &SwarmConfig, run_id: &str) {
    // TODO: Implement cancel via Acteon API + kill agent processes.
    warn!(run_id = %run_id, "cancel not yet implemented");
}
