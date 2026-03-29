use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};

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
        tracing::info!("no config file found at {}, using defaults", path.display());
        Ok(SwarmConfig::minimal())
    }
}

async fn cmd_plan_gather(config: &SwarmConfig, prompt: &str, output: &PathBuf) -> Result<()> {
    let engine = config.defaults.engine;
    println!("Gathering plan from {:?}...\n", engine);
    let plan = gather_plan(config, prompt).await?;

    let roles = RoleRegistry::with_config(engine, &config.roles);
    let warnings = validate_plan(&plan, &roles.names())?;

    for w in &warnings {
        println!("Warning: {}", w.message);
    }

    let json = serde_json::to_string_pretty(&plan)?;
    std::fs::write(output, &json)?;
    println!("\nPlan saved to {}", output.display());
    println!("Tasks: {}", plan.tasks.len());
    println!("Estimated actions: {}", plan.estimated_actions);
    println!("\nReview with: acteon-swarm plan show");
    println!("Approve with: acteon-swarm plan approve");

    Ok(())
}

fn cmd_plan_show(path: &PathBuf) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let plan: SwarmPlan = serde_json::from_str(&content)?;

    println!("Plan: {} ({})", plan.objective, plan.id);
    println!(
        "Status: {}",
        if plan.is_approved() {
            "APPROVED"
        } else {
            "PENDING"
        }
    );
    println!("Estimated actions: {}", plan.estimated_actions);
    println!("Success criteria:");
    for c in &plan.success_criteria {
        println!("  - {c}");
    }
    println!("\nTasks ({}):", plan.tasks.len());
    for task in &plan.tasks {
        let deps = if task.depends_on.is_empty() {
            String::new()
        } else {
            format!(" (depends: {})", task.depends_on.join(", "))
        };
        println!(
            "  [{}] {} (role: {}){deps}",
            task.id, task.name, task.assigned_role
        );
        for sub in &task.subtasks {
            println!("    - [{}] {}", sub.id, sub.name);
        }
    }

    Ok(())
}

fn cmd_plan_approve(path: &PathBuf) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut plan: SwarmPlan = serde_json::from_str(&content)?;

    if plan.is_approved() {
        println!("Plan is already approved.");
        return Ok(());
    }

    plan.approved_at = Some(Utc::now());
    let json = serde_json::to_string_pretty(&plan)?;
    std::fs::write(path, json)?;
    println!("Plan approved at {}", plan.approved_at.unwrap());

    Ok(())
}

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
            println!("Plan gathered. Approve with: acteon-swarm plan approve");
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

    let roles = RoleRegistry::with_config(config.defaults.engine, &config.roles);
    validate_plan(&plan, &roles.names())?;

    // Find the hook binary (should be next to this binary).
    let hooks_binary = std::env::current_exe()?
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("acteon-swarm-hook");

    println!("Starting swarm run...");
    let run = acteon_swarm::execute_swarm(&mut plan, config, &roles, &hooks_binary).await?;

    println!("\nSwarm run complete:");
    println!("  Status: {:?}", run.status);
    println!("  Agents spawned: {}", run.metrics.agents_spawned);
    println!("  Agents completed: {}", run.metrics.agents_completed);
    println!("  Agents failed: {}", run.metrics.agents_failed);
    println!("  Total actions: {}", run.metrics.total_actions);
    println!("  Refinements: {}", run.metrics.refinements);

    Ok(())
}

async fn cmd_status(config: &SwarmConfig, run_id: &str) -> Result<()> {
    let summary = acteon_swarm::acteon::audit_watcher::fetch_audit_summary(config, run_id).await?;

    println!("Swarm run: {run_id}");
    println!("  Total dispatched: {}", summary.total_dispatched);
    println!("  Executed: {}", summary.executed);
    println!("  Suppressed: {}", summary.suppressed);
    println!("  Throttled: {}", summary.throttled);
    println!("  Deduplicated: {}", summary.deduplicated);
    println!("  Pending approval: {}", summary.pending_approval);
    println!("  Quota exceeded: {}", summary.quota_exceeded);
    println!("  Rerouted: {}", summary.rerouted);

    Ok(())
}

fn cmd_cancel(_config: &SwarmConfig, run_id: &str) {
    // TODO: Implement cancel via Acteon API + kill agent processes.
    println!("Cancel not yet implemented for run {run_id}");
}
