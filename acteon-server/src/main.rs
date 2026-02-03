use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::sync::RwLock;
use tracing::info;

use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_rules_yaml::YamlFrontend;
use acteon_server::api::AppState;
use acteon_server::auth::AuthProvider;
use acteon_server::auth::crypto::{decrypt_auth_config, encrypt_value, parse_master_key};
use acteon_server::auth::watcher::AuthWatcher;
use acteon_server::config::ActeonConfig;
use acteon_server::ratelimit::{RateLimitFileConfig, RateLimiter};

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

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Encrypt a value for use in auth.toml. Reads plaintext from stdin.
    Encrypt,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing subscriber from RUST_LOG or default to info.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Handle subcommands.
    if let Some(Commands::Encrypt) = cli.command {
        return run_encrypt();
    }

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

    // Create the audit store if enabled.
    let audit_store = if config.audit.enabled {
        let store = acteon_server::audit_factory::create_audit_store(&config.audit).await?;
        info!(backend = %config.audit.backend, "audit store initialized");
        Some(store)
    } else {
        None
    };

    // Build the auth provider if enabled.
    let (auth_provider, _auth_watcher_handle) = if config.auth.enabled {
        let master_key_raw = std::env::var("ACTEON_AUTH_KEY")
            .map_err(|_| "ACTEON_AUTH_KEY environment variable is required when auth is enabled")?;
        let master_key = parse_master_key(&master_key_raw)
            .map_err(|e| format!("invalid ACTEON_AUTH_KEY: {e}"))?;

        let auth_path = config.auth.config_path.as_deref().unwrap_or("auth.toml");

        // Resolve relative to the config file's directory.
        let auth_path = if Path::new(auth_path).is_relative() {
            Path::new(&cli.config)
                .parent()
                .unwrap_or(Path::new("."))
                .join(auth_path)
        } else {
            Path::new(auth_path).to_path_buf()
        };

        let auth_contents = std::fs::read_to_string(&auth_path)
            .map_err(|e| format!("failed to read auth config at {}: {e}", auth_path.display()))?;
        let mut auth_config: acteon_server::auth::config::AuthFileConfig =
            toml::from_str(&auth_contents)
                .map_err(|e| format!("failed to parse auth config: {e}"))?;

        decrypt_auth_config(&mut auth_config, &master_key)?;

        let provider = Arc::new(AuthProvider::new(&auth_config, Arc::clone(&store))?);
        info!("auth provider initialized");

        // Spawn the auth watcher for hot-reload.
        let watcher_handle = if config.auth.watch.unwrap_or(true) {
            let watcher = AuthWatcher::new(Arc::clone(&provider), auth_path.clone(), master_key);
            Some(watcher.spawn())
        } else {
            None
        };

        (Some(provider), watcher_handle)
    } else {
        (None, None)
    };

    // Build the rate limiter if enabled.
    let rate_limiter = if config.rate_limit.enabled {
        let rl_path = config
            .rate_limit
            .config_path
            .as_deref()
            .unwrap_or("ratelimit.toml");

        // Resolve relative to the config file's directory.
        let rl_path = if Path::new(rl_path).is_relative() {
            Path::new(&cli.config)
                .parent()
                .unwrap_or(Path::new("."))
                .join(rl_path)
        } else {
            Path::new(rl_path).to_path_buf()
        };

        let rl_contents = std::fs::read_to_string(&rl_path).map_err(|e| {
            format!(
                "failed to read rate limit config at {}: {e}",
                rl_path.display()
            )
        })?;
        let rl_config: RateLimitFileConfig = toml::from_str(&rl_contents)
            .map_err(|e| format!("failed to parse rate limit config: {e}"))?;

        info!(path = %rl_path.display(), "rate limiter initialized");
        Some(Arc::new(RateLimiter::new(
            Arc::clone(&store),
            rl_config,
            config.rate_limit.on_error,
        )))
    } else {
        None
    };

    // Build the gateway.
    let mut builder = GatewayBuilder::new()
        .state(Arc::clone(&store))
        .lock(lock)
        .executor_config(exec_config)
        .dlq_enabled(config.executor.dlq_enabled);

    if let Some(ref audit) = audit_store {
        builder = builder
            .audit(Arc::clone(audit))
            .audit_store_payload(config.audit.store_payload);
        if let Some(ttl) = config.audit.ttl_seconds {
            builder = builder.audit_ttl_seconds(ttl);
        }
    }

    let mut gateway = builder.build()?;

    if config.executor.dlq_enabled {
        info!("dead-letter queue enabled");
    }

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

    // Spawn audit cleanup background task if audit is enabled.
    let _cleanup_handle = if let Some(ref audit) = audit_store {
        let interval = Duration::from_secs(config.audit.cleanup_interval_seconds);
        let store = Arc::clone(audit);
        Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            // The first tick completes immediately; skip it so we don't run
            // cleanup at startup.
            timer.tick().await;
            loop {
                timer.tick().await;
                match store.cleanup_expired().await {
                    Ok(0) => {}
                    Ok(n) => info!(removed = n, "audit cleanup removed expired records"),
                    Err(e) => tracing::warn!(error = %e, "audit cleanup failed"),
                }
            }
        }))
    } else {
        None
    };

    let gateway = Arc::new(RwLock::new(gateway));
    let state = AppState {
        gateway: Arc::clone(&gateway),
        audit: audit_store,
        auth: auth_provider,
        rate_limiter,
    };
    let app = acteon_server::api::router(state);

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

    // Wait for pending audit tasks to complete (with configurable timeout).
    let shutdown_timeout = Duration::from_secs(config.server.shutdown_timeout_seconds);
    info!(
        timeout_secs = config.server.shutdown_timeout_seconds,
        "waiting for pending audit tasks..."
    );
    let gw = gateway.read().await;
    if tokio::time::timeout(shutdown_timeout, gw.shutdown())
        .await
        .is_err()
    {
        tracing::warn!(
            timeout_secs = config.server.shutdown_timeout_seconds,
            "shutdown timeout exceeded, some audit tasks may be lost"
        );
    }

    info!("acteon-server shut down");
    Ok(())
}

/// Run the `encrypt` subcommand: read plaintext from stdin, output ENC[...] to stdout.
fn run_encrypt() -> Result<(), Box<dyn std::error::Error>> {
    let master_key_raw = std::env::var("ACTEON_AUTH_KEY")
        .map_err(|_| "ACTEON_AUTH_KEY environment variable is required for the encrypt command")?;
    let master_key =
        parse_master_key(&master_key_raw).map_err(|e| format!("invalid ACTEON_AUTH_KEY: {e}"))?;

    let mut plaintext = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut plaintext)?;
    let plaintext = plaintext.trim_end_matches('\n');

    let encrypted = encrypt_value(plaintext, &master_key)?;
    println!("{encrypted}");
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
