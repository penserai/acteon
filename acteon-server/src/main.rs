use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::sync::RwLock;
use tracing::info;

use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_rules_yaml::YamlFrontend;
use acteon_server::config::ActeonConfig;

/// Acteon gateway HTTP server.
#[derive(Parser, Debug)]
#[command(name = "acteon-server", about = "Standalone HTTP server for Acteon")]
struct Cli {
    /// Path to the TOML configuration file.
    #[arg(short, long, default_value = "acteon.toml")]
    config: String,

    /// Override the bind host.
    #[arg(long)]
    host: Option<String>,

    /// Override the bind port.
    #[arg(long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing subscriber from RUST_LOG or default to info.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Load configuration from TOML file, or use defaults if the file does not exist.
    let config: ActeonConfig = if Path::new(&cli.config).exists() {
        let contents = std::fs::read_to_string(&cli.config)?;
        toml::from_str(&contents)?
    } else {
        info!(
            path = %cli.config,
            "config file not found, using defaults"
        );
        toml::from_str("")?
    };

    // Build the executor config from TOML values.
    let mut exec_config = ExecutorConfig::default();
    if let Some(retries) = config.executor.max_retries {
        exec_config.max_retries = retries;
    }
    if let Some(timeout) = config.executor.timeout_seconds {
        exec_config.execution_timeout = Duration::from_secs(timeout);
    }
    if let Some(concurrent) = config.executor.max_concurrent {
        exec_config.max_concurrent = concurrent;
    }

    // Create the state backend.
    let (store, lock) = acteon_server::state_factory::create_state(&config.state).await?;

    // Build the gateway.
    let mut gateway = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .executor_config(exec_config)
        .build()?;

    // Optionally load rules from a directory.
    if let Some(ref dir) = config.rules.directory {
        let path = Path::new(dir);
        if path.is_dir() {
            let yaml_frontend = YamlFrontend;
            let frontends: Vec<&dyn acteon_rules::RuleFrontend> = vec![&yaml_frontend];
            let count = gateway.load_rules_from_directory(path, &frontends)?;
            info!(count, directory = %dir, "loaded rules from directory");
        } else {
            tracing::warn!(directory = %dir, "rules directory does not exist");
        }
    }

    let gateway = Arc::new(RwLock::new(gateway));
    let app = acteon_server::api::router(Arc::clone(&gateway));

    // Resolve the bind address (CLI overrides take precedence).
    let host = cli.host.unwrap_or(config.server.host);
    let port = cli.port.unwrap_or(config.server.port);
    let addr = format!("{host}:{port}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(address = %addr, "acteon-server listening");

    // Serve with graceful shutdown on SIGINT / SIGTERM.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("acteon-server shut down");
    Ok(())
}

/// Wait for SIGINT (Ctrl+C) or SIGTERM, then return to trigger graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => { info!("received SIGINT"); }
        () = terminate => { info!("received SIGTERM"); }
    }
}
