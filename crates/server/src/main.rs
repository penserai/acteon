use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::sync::RwLock;
use tracing::info;

use acteon_core::Action;
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_gateway::background::{BackgroundConfig, BackgroundProcessorBuilder};
use acteon_gateway::group_manager::GroupManager;
use acteon_rules_yaml::YamlFrontend;
use acteon_server::api::AppState;
use acteon_server::auth::AuthProvider;
use acteon_server::auth::crypto::{
    decrypt_auth_config, decrypt_value, encrypt_value, parse_master_key,
};
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

    // Parse the master key if available (used by auth and encrypted config values).
    let master_key = std::env::var("ACTEON_AUTH_KEY")
        .ok()
        .map(|raw| parse_master_key(&raw).map_err(|e| format!("invalid ACTEON_AUTH_KEY: {e}")))
        .transpose()?;

    // Build the auth provider if enabled.
    let (auth_provider, _auth_watcher_handle) = if config.auth.enabled {
        let auth_master_key = master_key
            .ok_or("ACTEON_AUTH_KEY environment variable is required when auth is enabled")?;

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

        decrypt_auth_config(&mut auth_config, &auth_master_key)?;

        let provider = Arc::new(AuthProvider::new(&auth_config, Arc::clone(&store))?);
        info!("auth provider initialized");

        // Spawn the auth watcher for hot-reload.
        let watcher_handle = if config.auth.watch.unwrap_or(true) {
            let watcher =
                AuthWatcher::new(Arc::clone(&provider), auth_path.clone(), auth_master_key);
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

    // Create a shared group manager for the gateway and background processor.
    let group_manager = Arc::new(GroupManager::new());

    // Build the gateway.
    let external_url = config
        .server
        .external_url
        .clone()
        .unwrap_or_else(|| format!("http://{}:{}", config.server.host, config.server.port));

    let mut builder = GatewayBuilder::new()
        .state(Arc::clone(&store))
        .lock(Arc::clone(&lock))
        .executor_config(exec_config)
        .dlq_enabled(config.executor.dlq_enabled)
        .group_manager(Arc::clone(&group_manager))
        .external_url(external_url);

    if let Some(ref key_configs) = config.server.approval_keys {
        let keys: Vec<acteon_gateway::ApprovalKey> = key_configs
            .iter()
            .map(|kc| {
                let secret = hex::decode(&kc.secret)
                    .map_err(|e| format!("invalid hex in approval_keys id={}: {e}", kc.id))?;
                Ok(acteon_gateway::ApprovalKey {
                    kid: kc.id.clone(),
                    secret,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let keyset = acteon_gateway::ApprovalKeySet::new(keys);
        builder = builder.approval_keys(keyset);
    } else if let Some(ref secret_hex) = config.server.approval_secret {
        let secret =
            hex::decode(secret_hex).map_err(|e| format!("invalid approval_secret hex: {e}"))?;
        builder = builder.approval_secret(secret);
    }

    // Wire LLM guardrail if enabled.
    if config.llm_guardrail.enabled {
        let api_key = require_decrypt(&config.llm_guardrail.api_key, master_key.as_ref())?;

        let mut llm_config = acteon_llm::LlmGuardrailConfig::new(
            &config.llm_guardrail.endpoint,
            &config.llm_guardrail.model,
            api_key,
        );
        if let Some(timeout) = config.llm_guardrail.timeout_seconds {
            llm_config = llm_config.with_timeout(timeout);
        }
        if let Some(temp) = config.llm_guardrail.temperature {
            llm_config = llm_config.with_temperature(temp);
        }
        if let Some(max) = config.llm_guardrail.max_tokens {
            llm_config = llm_config.with_max_tokens(max);
        }
        let evaluator = acteon_llm::HttpLlmEvaluator::new(llm_config)
            .map_err(|e| format!("failed to create LLM evaluator: {e}"))?;
        builder = builder
            .llm_evaluator(Arc::new(evaluator))
            .llm_policy(&config.llm_guardrail.policy)
            .llm_policies(config.llm_guardrail.policies.clone())
            .llm_fail_open(config.llm_guardrail.fail_open);
        info!(
            model = %config.llm_guardrail.model,
            fail_open = config.llm_guardrail.fail_open,
            "LLM guardrail enabled"
        );
    }

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

    // Recover pending groups from state store on startup.
    if config.background.enabled
        && !config.background.namespace.is_empty()
        && !config.background.tenant.is_empty()
    {
        match group_manager
            .recover_groups(
                store.as_ref(),
                &config.background.namespace,
                &config.background.tenant,
            )
            .await
        {
            Ok(count) if count > 0 => {
                info!(count, "recovered pending groups from state store");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "failed to recover groups from state store");
            }
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

    // Spawn background processor for group flushing and timeout processing.
    // This must be after gateway Arc is created so handlers can dispatch notifications.
    let _background_shutdown_tx = if config.background.enabled {
        let bg_config = BackgroundConfig {
            group_flush_interval: Duration::from_secs(
                config.background.group_flush_interval_seconds,
            ),
            timeout_check_interval: Duration::from_secs(
                config.background.timeout_check_interval_seconds,
            ),
            cleanup_interval: Duration::from_secs(config.background.cleanup_interval_seconds),
            enable_group_flush: config.background.enable_group_flush,
            enable_timeout_processing: config.background.enable_timeout_processing,
            enable_approval_retry: config.background.enable_approval_retry,
            namespace: config.background.namespace.clone(),
            tenant: config.background.tenant.clone(),
        };

        // Create channels for receiving flush, timeout, and approval retry events.
        let (flush_tx, mut flush_rx) = tokio::sync::mpsc::channel(100);
        let (timeout_tx, mut timeout_rx) = tokio::sync::mpsc::channel(100);
        let (approval_retry_tx, mut approval_retry_rx) = tokio::sync::mpsc::channel(100);

        let mut bg_builder = BackgroundProcessorBuilder::new()
            .config(bg_config)
            .group_manager(Arc::clone(&group_manager))
            .state(Arc::clone(&store))
            .group_flush_channel(flush_tx)
            .timeout_channel(timeout_tx);

        if config.background.enable_approval_retry {
            bg_builder = bg_builder.approval_retry_channel(approval_retry_tx);
        }

        let (mut processor, shutdown_tx) = bg_builder
            .build()
            .map_err(|e| format!("failed to build background processor: {e}"))?;

        // Spawn the background processor.
        tokio::spawn(async move {
            processor.run().await;
        });

        // Spawn consumer for group flush events.
        // Creates a summary notification action and dispatches it through the gateway.
        let flush_gateway = Arc::clone(&gateway);
        tokio::spawn(async move {
            while let Some(event) = flush_rx.recv().await {
                let group = &event.group;
                info!(
                    group_id = %group.group_id,
                    event_count = group.size(),
                    flushed_at = %event.flushed_at,
                    "group flushed - dispatching notification"
                );

                // Build a summary notification action from the grouped events.
                // Uses the first event's metadata and aggregates the payloads.
                let payloads: Vec<_> = group.events.iter().map(|e| e.payload.clone()).collect();
                let summary_payload = serde_json::json!({
                    "group_id": group.group_id,
                    "group_key": group.group_key,
                    "event_count": group.size(),
                    "events": payloads,
                    "labels": group.labels,
                    "flushed_at": event.flushed_at.to_rfc3339(),
                });

                // Extract namespace/tenant from labels or use defaults.
                let namespace = group
                    .labels
                    .get("namespace")
                    .cloned()
                    .unwrap_or_else(|| "default".to_string());
                let tenant = group
                    .labels
                    .get("tenant")
                    .cloned()
                    .unwrap_or_else(|| "default".to_string());
                let provider = group
                    .labels
                    .get("provider")
                    .cloned()
                    .unwrap_or_else(|| "webhook".to_string());

                let action = Action::new(
                    namespace.as_str(),
                    tenant.as_str(),
                    provider.as_str(),
                    "group_notification",
                    summary_payload,
                );

                // Dispatch the notification through the gateway.
                let gw = flush_gateway.read().await;
                match gw.dispatch(action, None).await {
                    Ok(outcome) => {
                        info!(
                            group_id = %group.group_id,
                            ?outcome,
                            "group notification dispatched"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            group_id = %group.group_id,
                            error = %e,
                            "failed to dispatch group notification"
                        );
                    }
                }
            }
        });

        // Spawn consumer for timeout events.
        // Creates a timeout notification action and dispatches it through the gateway.
        let timeout_gateway = Arc::clone(&gateway);
        let timeout_namespace = config.background.namespace.clone();
        let timeout_tenant = config.background.tenant.clone();
        tokio::spawn(async move {
            while let Some(event) = timeout_rx.recv().await {
                info!(
                    fingerprint = %event.fingerprint,
                    state_machine = %event.state_machine,
                    previous_state = %event.previous_state,
                    new_state = %event.new_state,
                    fired_at = %event.fired_at,
                    "timeout fired - dispatching notification"
                );

                // Build a timeout notification action.
                let timeout_payload = serde_json::json!({
                    "fingerprint": event.fingerprint,
                    "state_machine": event.state_machine,
                    "previous_state": event.previous_state,
                    "new_state": event.new_state,
                    "fired_at": event.fired_at.to_rfc3339(),
                });

                let action = Action::new(
                    timeout_namespace.as_str(),
                    timeout_tenant.as_str(),
                    "webhook",
                    "timeout_notification",
                    timeout_payload,
                );

                // Dispatch the notification through the gateway.
                let gw = timeout_gateway.read().await;
                match gw.dispatch(action, None).await {
                    Ok(outcome) => {
                        info!(
                            fingerprint = %event.fingerprint,
                            ?outcome,
                            "timeout notification dispatched"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            fingerprint = %event.fingerprint,
                            error = %e,
                            "failed to dispatch timeout notification"
                        );
                    }
                }
            }
        });

        // Spawn consumer for approval retry events.
        // Retries sending the notification for approvals where it previously failed.
        let retry_gateway = Arc::clone(&gateway);
        tokio::spawn(async move {
            while let Some(event) = approval_retry_rx.recv().await {
                info!(
                    approval_id = %event.approval_id,
                    namespace = %event.namespace,
                    tenant = %event.tenant,
                    "retrying approval notification"
                );

                let gw = retry_gateway.read().await;
                match gw
                    .retry_approval_notification(
                        &event.namespace,
                        &event.tenant,
                        &event.approval_id,
                    )
                    .await
                {
                    Ok(true) => {
                        info!(
                            approval_id = %event.approval_id,
                            "approval notification retry succeeded"
                        );
                    }
                    Ok(false) => {
                        tracing::debug!(
                            approval_id = %event.approval_id,
                            "approval notification retry skipped (no longer eligible)"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            approval_id = %event.approval_id,
                            error = %e,
                            "approval notification retry failed"
                        );
                    }
                }
            }
        });

        info!("background processor started");
        Some(shutdown_tx)
    } else {
        None
    };

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

/// Decrypt a config value, requiring `ACTEON_AUTH_KEY` if the value is encrypted.
///
/// - `ENC[...]` values are decrypted using the master key (error if key is missing).
/// - Plain values are returned as-is regardless of whether a key is available.
fn require_decrypt(
    value: &str,
    master_key: Option<&[u8; 32]>,
) -> Result<String, Box<dyn std::error::Error>> {
    if value.trim().starts_with("ENC[") {
        let mk = master_key.ok_or(
            "ACTEON_AUTH_KEY environment variable is required to decrypt ENC[...] config values",
        )?;
        Ok(decrypt_value(value, mk)?)
    } else {
        Ok(value.to_owned())
    }
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
