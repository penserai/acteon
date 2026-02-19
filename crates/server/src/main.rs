use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::sync::RwLock;
use tracing::info;

use acteon_core::{
    Action, BranchCondition, BranchOperator, ChainConfig, ChainFailurePolicy,
    ChainNotificationTarget, ChainStepConfig, StepFailurePolicy, StreamEvent, StreamEventType,
};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_gateway::background::{BackgroundConfig, BackgroundProcessorBuilder};
use acteon_gateway::group_manager::GroupManager;
use acteon_rules_yaml::YamlFrontend;
use acteon_server::api::AppState;
use acteon_server::auth::AuthProvider;
use acteon_server::auth::crypto::{
    ExposeSecret, MasterKey, decrypt_auth_config, decrypt_value, encrypt_value, parse_master_key,
};
use acteon_server::auth::watcher::AuthWatcher;
use acteon_server::config::{ActeonConfig, ConfigSnapshot};
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
    /// Run database migrations for configured state and audit backends, then exit.
    Migrate,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Handle subcommands (need basic tracing before config is loaded).
    if let Some(Commands::Encrypt) = cli.command {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
        return run_encrypt();
    }

    // Load configuration from TOML file, or use defaults if the file does not exist.
    let config: ActeonConfig = if Path::new(&cli.config).exists() {
        let contents = std::fs::read_to_string(&cli.config)?;
        toml::from_str(&contents)?
    } else {
        toml::from_str("")?
    };

    // Handle the `migrate` subcommand before full tracing/OTel init.
    if let Some(Commands::Migrate) = cli.command {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
        return run_migrate(&config).await;
    }

    // Initialize tracing subscriber (with optional OpenTelemetry layer).
    // Must happen after config is loaded so we know whether OTel is enabled,
    // but before any tracing calls.
    let telemetry_guard = acteon_server::telemetry::init(&config.telemetry);

    if !Path::new(&cli.config).exists() {
        info!(
            path = %cli.config,
            "config file not found, using defaults"
        );
    }

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
            .as_ref()
            .ok_or("ACTEON_AUTH_KEY environment variable is required when auth is enabled")?
            .clone();

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

    // Parse the payload encryption key(s) if available.
    //
    // Key rotation support:
    //   ACTEON_PAYLOAD_KEYS="kid:hex,kid:hex,..."  (first key encrypts, all decrypt)
    //   ACTEON_PAYLOAD_KEY="hex"                    (single-key backward compat)
    let payload_encryptor: Option<Arc<acteon_crypto::PayloadEncryptor>> = if config
        .encryption
        .enabled
    {
        if let Ok(raw_keys) = std::env::var("ACTEON_PAYLOAD_KEYS") {
            let mut entries = Vec::new();
            for pair in raw_keys.split(',') {
                let pair = pair.trim();
                if pair.is_empty() {
                    continue;
                }
                let (kid, hex) = pair.split_once(':').ok_or_else(|| {
                    format!("invalid ACTEON_PAYLOAD_KEYS entry (expected kid:hex): {pair}")
                })?;
                let key = acteon_crypto::parse_master_key(hex)
                    .map_err(|e| format!("invalid key for kid={kid}: {e}"))?;
                entries.push(acteon_crypto::PayloadKeyEntry {
                    kid: kid.to_owned(),
                    key,
                });
            }
            if entries.is_empty() {
                return Err("ACTEON_PAYLOAD_KEYS is set but contains no valid key entries".into());
            }
            info!(
                key_count = entries.len(),
                primary_kid = %entries[0].kid,
                "payload encryption at rest enabled (multi-key)"
            );
            Some(Arc::new(acteon_crypto::PayloadEncryptor::with_keys(
                entries,
            )))
        } else if let Ok(raw) = std::env::var("ACTEON_PAYLOAD_KEY") {
            let key = acteon_crypto::parse_master_key(&raw)
                .map_err(|e| format!("invalid ACTEON_PAYLOAD_KEY: {e}"))?;
            let enc = Arc::new(acteon_crypto::PayloadEncryptor::new(key));
            info!("payload encryption at rest enabled");
            Some(enc)
        } else {
            return Err(
                    "ACTEON_PAYLOAD_KEY or ACTEON_PAYLOAD_KEYS environment variable is required when encryption.enabled = true".into(),
                );
        }
    } else {
        None
    };

    // Wrap audit store with encryption if enabled.
    // Wrapping order: EncryptingAuditStore(RedactingAuditStore(Inner))
    // Redaction runs first (on plaintext), then encryption.
    let audit_store: Option<Arc<dyn acteon_audit::store::AuditStore>> = if let Some(store) =
        audit_store
    {
        // Wrap with redaction if configured.
        let store = if config.audit.redact.enabled {
            let redact_config = acteon_audit::RedactConfig::new(config.audit.redact.fields.clone())
                .with_placeholder(&config.audit.redact.placeholder);
            Arc::new(acteon_audit::RedactingAuditStore::new(
                store,
                &redact_config,
            )) as Arc<dyn acteon_audit::store::AuditStore>
        } else {
            store
        };

        // Wrap with encryption if enabled.
        if let Some(ref enc) = payload_encryptor {
            Some(Arc::new(acteon_audit::EncryptingAuditStore::new(
                store,
                Arc::clone(enc),
            )) as Arc<dyn acteon_audit::store::AuditStore>)
        } else {
            Some(store)
        }
    } else {
        None
    };

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

    if let Some(ref enc) = payload_encryptor {
        builder = builder.payload_encryptor(Arc::clone(enc));
    }

    if let Some(ref tz) = config.rules.default_timezone {
        builder = builder.default_timezone(tz);
    }

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

    // Wire embedding provider for semantic routing if enabled.
    let embedding_bridge: Option<Arc<acteon_embedding::EmbeddingBridge>> =
        if config.embedding.enabled {
            let api_key = require_decrypt(&config.embedding.api_key, master_key.as_ref())?;

            let embedding_config = acteon_embedding::EmbeddingConfig {
                endpoint: config.embedding.endpoint.clone(),
                model: config.embedding.model.clone(),
                api_key,
                timeout_seconds: config.embedding.timeout_seconds,
            };
            let provider = acteon_embedding::HttpEmbeddingProvider::new(embedding_config)
                .map_err(|e| format!("failed to create embedding provider: {e}"))?;
            let bridge_config = acteon_embedding::EmbeddingBridgeConfig {
                topic_cache_capacity: config.embedding.topic_cache_capacity,
                topic_cache_ttl_seconds: config.embedding.topic_cache_ttl_seconds,
                text_cache_capacity: config.embedding.text_cache_capacity,
                text_cache_ttl_seconds: config.embedding.text_cache_ttl_seconds,
                fail_open: config.embedding.fail_open,
            };
            let bridge = Arc::new(acteon_embedding::EmbeddingBridge::new(
                Arc::new(provider),
                bridge_config,
            ));
            builder = builder.embedding_support(
                Arc::clone(&bridge) as Arc<dyn acteon_rules::EmbeddingEvalSupport>
            );
            info!(
                model = %config.embedding.model,
                fail_open = config.embedding.fail_open,
                "embedding provider enabled for semantic routing"
            );
            Some(bridge)
        } else {
            None
        };

    // Wire WASM plugin runtime if enabled.
    if config.wasm.enabled {
        let wasm_runtime_config = acteon_wasm_runtime::config::WasmRuntimeConfig {
            enabled: true,
            plugin_dir: config.wasm.plugin_dir.clone(),
            default_memory_limit_bytes: config.wasm.default_memory_limit_bytes,
            default_timeout_ms: config.wasm.default_timeout_ms,
        };
        let registry = acteon_wasm_runtime::WasmPluginRegistry::new(wasm_runtime_config)
            .map_err(|e| format!("failed to create WASM runtime: {e}"))?;

        // Load plugins from the configured directory.
        let loaded = registry
            .load_plugin_dir()
            .map_err(|e| format!("failed to load WASM plugins: {e}"))?;

        let shared = acteon_wasm_runtime::SharedWasmRegistry::new(registry);
        builder = builder.wasm_runtime(Arc::new(shared));
        info!(
            count = loaded,
            dir = config.wasm.plugin_dir.as_deref().unwrap_or("(none)"),
            "WASM plugin runtime enabled"
        );
    }

    // Wire compliance configuration.
    if config.compliance.is_active() {
        let compliance = config.compliance.to_compliance_config();

        // Validate backend compatibility: hash chaining requires a backend
        // that supports atomic sequence number uniqueness for optimistic
        // concurrency in multi-replica deployments. Postgres (UNIQUE
        // constraint), DynamoDB (conditional writes), and memory (dev/test)
        // are supported; ClickHouse and Elasticsearch lack synchronous
        // unique constraints.
        if compliance.hash_chain {
            let backend = config.audit.backend.as_str();
            match backend {
                "postgres" | "memory" | "dynamodb" => {}
                other => {
                    return Err(format!(
                        "compliance hash_chain requires the 'postgres' or 'dynamodb' \
                         audit backend for multi-replica correctness, but the \
                         configured backend is '{other}'. ClickHouse and Elasticsearch \
                         do not support synchronous unique constraints needed for \
                         hash chain integrity. Either switch to 'postgres'/'dynamodb' \
                         or disable hash_chain in [compliance]."
                    )
                    .into());
                }
            }
        }

        info!(
            mode = %compliance.mode,
            sync_audit_writes = compliance.sync_audit_writes,
            immutable_audit = compliance.immutable_audit,
            hash_chain = compliance.hash_chain,
            "compliance mode enabled"
        );
        builder = builder.compliance_config(compliance);
    }

    // Wire task chain definitions.
    for chain_toml in &config.chains.definitions {
        let on_failure = match chain_toml.on_failure.as_deref() {
            Some("abort_no_dlq") => ChainFailurePolicy::AbortNoDlq,
            _ => ChainFailurePolicy::Abort,
        };
        let mut chain_config = ChainConfig::new(&chain_toml.name).with_on_failure(on_failure);
        if let Some(timeout) = chain_toml.timeout_seconds {
            chain_config = chain_config.with_timeout(timeout);
        }
        if let Some(ref on_cancel) = chain_toml.on_cancel {
            chain_config = chain_config.with_on_cancel(ChainNotificationTarget {
                provider: on_cancel.provider.clone(),
                action_type: on_cancel.action_type.clone(),
            });
        }
        for step_toml in &chain_toml.steps {
            let mut step = if let Some(ref sub_chain_name) = step_toml.sub_chain {
                ChainStepConfig::new_sub_chain(&step_toml.name, sub_chain_name)
            } else {
                ChainStepConfig::new(
                    &step_toml.name,
                    step_toml.provider.as_deref().unwrap_or(""),
                    step_toml.action_type.as_deref().unwrap_or(""),
                    step_toml.payload_template.clone(),
                )
            };
            if let Some(ref policy) = step_toml.on_failure {
                let step_policy = match policy.as_str() {
                    "skip" => StepFailurePolicy::Skip,
                    "dlq" => StepFailurePolicy::Dlq,
                    _ => StepFailurePolicy::Abort,
                };
                step = step.with_on_failure(step_policy);
            }
            if let Some(delay) = step_toml.delay_seconds {
                step = step.with_delay(delay);
            }
            for branch_toml in &step_toml.branches {
                let operator = match branch_toml.operator.as_str() {
                    "neq" => BranchOperator::Neq,
                    "contains" => BranchOperator::Contains,
                    "exists" => BranchOperator::Exists,
                    "gt" => BranchOperator::Gt,
                    "lt" => BranchOperator::Lt,
                    "gte" => BranchOperator::Gte,
                    "lte" => BranchOperator::Lte,
                    _ => BranchOperator::Eq,
                };
                step = step.with_branch(BranchCondition::new(
                    &branch_toml.field,
                    operator,
                    branch_toml.value.clone(),
                    &branch_toml.target,
                ));
            }
            if let Some(ref default_next) = step_toml.default_next {
                step = step.with_default_next(default_next);
            }
            chain_config = chain_config.with_step(step);
        }
        builder = builder.chain(chain_config);
    }
    if !config.chains.definitions.is_empty() {
        builder = builder.completed_chain_ttl(Duration::from_secs(
            config.chains.completed_chain_ttl_seconds,
        ));
        info!(
            count = config.chains.definitions.len(),
            "task chains registered"
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

    // Wire circuit breakers if enabled.
    if config.circuit_breaker.enabled {
        let default_cb = acteon_gateway::CircuitBreakerConfig {
            failure_threshold: config.circuit_breaker.failure_threshold,
            success_threshold: config.circuit_breaker.success_threshold,
            recovery_timeout: Duration::from_secs(config.circuit_breaker.recovery_timeout_seconds),
            fallback_provider: None,
        };
        builder = builder.circuit_breaker(default_cb);

        for (provider, override_cfg) in &config.circuit_breaker.providers {
            let provider_cb = acteon_gateway::CircuitBreakerConfig {
                failure_threshold: override_cfg
                    .failure_threshold
                    .unwrap_or(config.circuit_breaker.failure_threshold),
                success_threshold: override_cfg
                    .success_threshold
                    .unwrap_or(config.circuit_breaker.success_threshold),
                recovery_timeout: Duration::from_secs(
                    override_cfg
                        .recovery_timeout_seconds
                        .unwrap_or(config.circuit_breaker.recovery_timeout_seconds),
                ),
                fallback_provider: override_cfg.fallback_provider.clone(),
            };
            builder = builder.circuit_breaker_provider(provider, provider_cb);
        }

        info!(
            failure_threshold = config.circuit_breaker.failure_threshold,
            recovery_timeout_seconds = config.circuit_breaker.recovery_timeout_seconds,
            overrides = config.circuit_breaker.providers.len(),
            "circuit breakers enabled"
        );
    }

    // Register providers from config.
    for provider_cfg in &config.providers {
        let provider: std::sync::Arc<dyn acteon_provider::DynProvider> = match provider_cfg
            .provider_type
            .as_str()
        {
            "webhook" => {
                let url = provider_cfg.url.as_deref().ok_or_else(|| {
                    format!(
                        "provider '{}': webhook type requires a 'url' field",
                        provider_cfg.name
                    )
                })?;
                let mut wp =
                    acteon_provider::webhook::WebhookProvider::new(&provider_cfg.name, url);
                if !provider_cfg.headers.is_empty() {
                    wp = wp.with_headers(provider_cfg.headers.clone());
                }
                std::sync::Arc::new(wp)
            }
            "log" => std::sync::Arc::new(acteon_provider::LogProvider::new(&provider_cfg.name)),
            "twilio" => {
                let account_sid = provider_cfg.account_sid.as_deref().ok_or_else(|| {
                    format!(
                        "provider '{}': twilio type requires an 'account_sid' field",
                        provider_cfg.name
                    )
                })?;
                let auth_token_raw = provider_cfg.auth_token.as_deref().ok_or_else(|| {
                    format!(
                        "provider '{}': twilio type requires an 'auth_token' field",
                        provider_cfg.name
                    )
                })?;
                let auth_token = require_decrypt(auth_token_raw, master_key.as_ref())?;
                let mut twilio_config = acteon_twilio::TwilioConfig::new(account_sid, auth_token);
                if let Some(ref from) = provider_cfg.from_number {
                    twilio_config = twilio_config.with_from_number(from);
                }
                std::sync::Arc::new(acteon_twilio::TwilioProvider::new(twilio_config))
            }
            "teams" => {
                let webhook_url = provider_cfg
                    .webhook_url
                    .as_deref()
                    .or(provider_cfg.url.as_deref())
                    .ok_or_else(|| {
                        format!(
                            "provider '{}': teams type requires a 'webhook_url' field",
                            provider_cfg.name
                        )
                    })?;
                let teams_config = acteon_teams::TeamsConfig::new(webhook_url);
                std::sync::Arc::new(acteon_teams::TeamsProvider::new(teams_config))
            }
            "discord" => {
                let webhook_url = provider_cfg
                    .webhook_url
                    .as_deref()
                    .or(provider_cfg.url.as_deref())
                    .ok_or_else(|| {
                        format!(
                            "provider '{}': discord type requires a 'webhook_url' field",
                            provider_cfg.name
                        )
                    })?;
                let mut discord_config = acteon_discord::DiscordConfig::new(webhook_url);
                if let Some(ref username) = provider_cfg.default_channel {
                    discord_config = discord_config.with_default_username(username);
                }
                std::sync::Arc::new(acteon_discord::DiscordProvider::new(discord_config))
            }
            "email" => {
                let from_address = provider_cfg.from_address.as_deref().ok_or_else(|| {
                    format!(
                        "provider '{}': email type requires a 'from_address' field",
                        provider_cfg.name
                    )
                })?;
                let backend = provider_cfg.email_backend.as_deref().unwrap_or("smtp");
                match backend {
                    "smtp" => {
                        let smtp_host = provider_cfg.smtp_host.as_deref().ok_or_else(|| {
                            format!(
                                "provider '{}': email/smtp backend requires a 'smtp_host' field",
                                provider_cfg.name
                            )
                        })?;
                        let mut email_config =
                            acteon_email::EmailConfig::new(smtp_host, from_address);
                        if let Some(port) = provider_cfg.smtp_port {
                            email_config = email_config.with_port(port);
                        }
                        if let Some(tls) = provider_cfg.tls {
                            email_config = email_config.with_tls(tls);
                        }
                        if let (Some(user), Some(pass_raw)) =
                            (&provider_cfg.username, &provider_cfg.password)
                        {
                            let pass = require_decrypt(pass_raw, master_key.as_ref())?;
                            email_config = email_config.with_credentials(user, pass);
                        }
                        std::sync::Arc::new(
                            acteon_email::EmailProvider::new(&email_config)
                                .map_err(|e| format!("provider '{}': {e}", provider_cfg.name))?,
                        )
                    }
                    "ses" => {
                        let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                        let mut email_config = acteon_email::EmailConfig::ses(region, from_address);
                        if let Some(ref url) = provider_cfg.aws_endpoint_url {
                            email_config = email_config.with_aws_endpoint_url(url);
                        }
                        if let Some(ref arn) = provider_cfg.aws_role_arn {
                            email_config = email_config.with_aws_role_arn(arn);
                        }
                        if let Some(ref set) = provider_cfg.ses_configuration_set {
                            email_config = email_config.with_ses_configuration_set(set);
                        }
                        if let Some(ref name) = provider_cfg.aws_session_name {
                            email_config = email_config.with_aws_session_name(name);
                        }
                        if let Some(ref ext_id) = provider_cfg.aws_external_id {
                            email_config = email_config.with_aws_external_id(ext_id);
                        }
                        std::sync::Arc::new(acteon_email::EmailProvider::ses(&email_config).await)
                    }
                    other => {
                        return Err(format!(
                            "provider '{}': unknown email backend '{other}' (expected 'smtp' or 'ses')",
                            provider_cfg.name
                        )
                        .into());
                    }
                }
            }
            "aws-sns" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut sns_config = acteon_aws::SnsConfig::new(region);
                if let Some(ref arn) = provider_cfg.topic_arn {
                    sns_config = sns_config.with_topic_arn(arn);
                }
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    sns_config = sns_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    sns_config = sns_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    sns_config = sns_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    sns_config = sns_config.with_external_id(ext_id);
                }
                std::sync::Arc::new(acteon_aws::SnsProvider::new(sns_config).await)
            }
            "aws-lambda" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut lambda_config = acteon_aws::LambdaConfig::new(region);
                if let Some(ref name) = provider_cfg.function_name {
                    lambda_config = lambda_config.with_function_name(name);
                }
                if let Some(ref q) = provider_cfg.qualifier {
                    lambda_config = lambda_config.with_qualifier(q);
                }
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    lambda_config = lambda_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    lambda_config = lambda_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    lambda_config = lambda_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    lambda_config = lambda_config.with_external_id(ext_id);
                }
                std::sync::Arc::new(acteon_aws::LambdaProvider::new(lambda_config).await)
            }
            "aws-eventbridge" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut eb_config = acteon_aws::EventBridgeConfig::new(region);
                if let Some(ref bus) = provider_cfg.event_bus_name {
                    eb_config = eb_config.with_event_bus_name(bus);
                }
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    eb_config = eb_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    eb_config = eb_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    eb_config = eb_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    eb_config = eb_config.with_external_id(ext_id);
                }
                std::sync::Arc::new(acteon_aws::EventBridgeProvider::new(eb_config).await)
            }
            "aws-sqs" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut sqs_config = acteon_aws::SqsConfig::new(region);
                if let Some(ref url) = provider_cfg.queue_url {
                    sqs_config = sqs_config.with_queue_url(url);
                }
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    sqs_config = sqs_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    sqs_config = sqs_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    sqs_config = sqs_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    sqs_config = sqs_config.with_external_id(ext_id);
                }
                std::sync::Arc::new(acteon_aws::SqsProvider::new(sqs_config).await)
            }
            "aws-s3" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut s3_config = acteon_aws::S3Config::new(region);
                if let Some(ref bucket) = provider_cfg.bucket_name {
                    s3_config = s3_config.with_bucket(bucket);
                }
                if let Some(ref prefix) = provider_cfg.object_prefix {
                    s3_config = s3_config.with_prefix(prefix);
                }
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    s3_config = s3_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    s3_config = s3_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    s3_config = s3_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    s3_config = s3_config.with_external_id(ext_id);
                }
                std::sync::Arc::new(acteon_aws::S3Provider::new(s3_config).await)
            }
            "aws-ec2" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut ec2_config = acteon_aws::Ec2Config::new(region);
                if let Some(ref ids) = provider_cfg.default_security_group_ids {
                    ec2_config = ec2_config.with_default_security_group_ids(ids.clone());
                }
                if let Some(ref sid) = provider_cfg.default_subnet_id {
                    ec2_config = ec2_config.with_default_subnet_id(sid);
                }
                if let Some(ref kn) = provider_cfg.default_key_name {
                    ec2_config = ec2_config.with_default_key_name(kn);
                }
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    ec2_config = ec2_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    ec2_config = ec2_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    ec2_config = ec2_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    ec2_config = ec2_config.with_external_id(ext_id);
                }
                let ec2 = std::sync::Arc::new(acteon_aws::Ec2Provider::new(ec2_config).await);
                builder = builder.resource_lookup(
                    provider_cfg.name.clone(),
                    std::sync::Arc::clone(&ec2)
                        as std::sync::Arc<dyn acteon_provider::ResourceLookup>,
                );
                ec2
            }
            "aws-autoscaling" => {
                let region = provider_cfg.aws_region.as_deref().unwrap_or("us-east-1");
                let mut asg_config = acteon_aws::AutoScalingConfig::new(region);
                if let Some(ref url) = provider_cfg.aws_endpoint_url {
                    asg_config = asg_config.with_endpoint_url(url);
                }
                if let Some(ref arn) = provider_cfg.aws_role_arn {
                    asg_config = asg_config.with_role_arn(arn);
                }
                if let Some(ref name) = provider_cfg.aws_session_name {
                    asg_config = asg_config.with_session_name(name);
                }
                if let Some(ref ext_id) = provider_cfg.aws_external_id {
                    asg_config = asg_config.with_external_id(ext_id);
                }
                let asg =
                    std::sync::Arc::new(acteon_aws::AutoScalingProvider::new(asg_config).await);
                builder = builder.resource_lookup(
                    provider_cfg.name.clone(),
                    std::sync::Arc::clone(&asg)
                        as std::sync::Arc<dyn acteon_provider::ResourceLookup>,
                );
                asg
            }
            other => {
                return Err(format!(
                    "provider '{}': unknown type '{other}' (expected 'webhook', 'log', 'twilio', \
                         'teams', 'discord', 'email', 'aws-sns', 'aws-lambda', 'aws-eventbridge', \
                         'aws-sqs', 'aws-s3', 'aws-ec2', or 'aws-autoscaling')",
                    provider_cfg.name
                )
                .into());
            }
        };
        builder = builder.provider(provider);
    }
    if !config.providers.is_empty() {
        info!(
            count = config.providers.len(),
            "providers registered from config"
        );
    }

    // Wire enrichment configs.
    for enrichment_toml in &config.enrichments {
        let enrichment_config = acteon_core::EnrichmentConfig {
            name: enrichment_toml.name.clone(),
            namespace: enrichment_toml.namespace.clone(),
            tenant: enrichment_toml.tenant.clone(),
            action_type: enrichment_toml.action_type.clone(),
            provider: enrichment_toml.provider.clone(),
            lookup_provider: enrichment_toml.lookup_provider.clone(),
            resource_type: enrichment_toml.resource_type.clone(),
            params: enrichment_toml.params.clone(),
            merge_key: enrichment_toml.merge_key.clone(),
            timeout_seconds: enrichment_toml.timeout_seconds,
            failure_policy: enrichment_toml.failure_policy,
        };
        builder = builder.enrichment(enrichment_config);
    }
    if !config.enrichments.is_empty() {
        info!(
            count = config.enrichments.len(),
            "pre-dispatch enrichments configured"
        );
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

    // Load quota policies from state store on startup.
    if config.quotas.enabled {
        match store.scan_keys_by_kind(acteon_state::KeyKind::Quota).await {
            Ok(entries) => {
                let mut count = 0usize;
                for (_key, value) in entries {
                    if let Ok(policy) = serde_json::from_str::<acteon_core::QuotaPolicy>(&value) {
                        gateway.set_quota_policy(policy);
                        count += 1;
                    }
                }
                if count > 0 {
                    info!(count, "loaded quota policies from state store");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to load quota policies from state store");
            }
        }
    }

    // Load retention policies from state store on startup.
    match store
        .scan_keys_by_kind(acteon_state::KeyKind::Retention)
        .await
    {
        Ok(entries) => {
            let mut count = 0usize;
            for (_key, value) in entries {
                if let Ok(policy) = serde_json::from_str::<acteon_core::RetentionPolicy>(&value) {
                    gateway.set_retention_policy(policy);
                    count += 1;
                }
            }
            if count > 0 {
                info!(count, "loaded retention policies from state store");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load retention policies from state store");
        }
    }

    // Pre-warm the embedding topic cache with topics from loaded rules.
    if let Some(ref bridge) = embedding_bridge {
        let topics: Vec<&str> = gateway
            .rules()
            .iter()
            .flat_map(|r| r.condition.semantic_topics())
            .collect();
        if !topics.is_empty() {
            info!(count = topics.len(), "pre-warming embedding topic cache");
            bridge.warm_topics(&topics).await;
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
                payload_encryptor.as_deref(),
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
            enable_chain_advancement: !config.chains.definitions.is_empty(),
            chain_check_interval: Duration::from_secs(5),
            enable_scheduled_actions: config.background.enable_scheduled_actions,
            scheduled_check_interval: Duration::from_secs(
                config.background.scheduled_check_interval_seconds,
            ),
            enable_recurring_actions: config.background.enable_recurring_actions,
            recurring_check_interval: Duration::from_secs(
                config.background.recurring_check_interval_seconds,
            ),
            enable_retention_reaper: config.background.enable_retention_reaper,
            retention_check_interval: Duration::from_secs(
                config.background.retention_check_interval_seconds,
            ),
            namespace: config.background.namespace.clone(),
            tenant: config.background.tenant.clone(),
        };

        // Create channels for receiving flush, timeout, approval retry, and chain advance events.
        let (flush_tx, mut flush_rx) = tokio::sync::mpsc::channel(100);
        let (timeout_tx, mut timeout_rx) = tokio::sync::mpsc::channel(100);
        let (approval_retry_tx, mut approval_retry_rx) = tokio::sync::mpsc::channel(100);
        let (chain_advance_tx, mut chain_advance_rx) = tokio::sync::mpsc::channel(100);
        let (scheduled_action_tx, mut scheduled_action_rx) = tokio::sync::mpsc::channel(100);
        let (recurring_action_tx, mut recurring_action_rx) = tokio::sync::mpsc::channel(100);

        let mut bg_builder = BackgroundProcessorBuilder::new()
            .config(bg_config)
            .metrics(gateway.read().await.metrics_arc())
            .group_manager(Arc::clone(&group_manager))
            .state(Arc::clone(&store))
            .group_flush_channel(flush_tx)
            .timeout_channel(timeout_tx);

        if config.background.enable_approval_retry {
            bg_builder = bg_builder.approval_retry_channel(approval_retry_tx);
        }

        if !config.chains.definitions.is_empty() {
            bg_builder = bg_builder.chain_advance_channel(chain_advance_tx);
        }

        if config.background.enable_scheduled_actions {
            bg_builder = bg_builder.scheduled_action_channel(scheduled_action_tx);
        }

        if config.background.enable_recurring_actions {
            bg_builder = bg_builder.recurring_action_channel(recurring_action_tx);
        }

        if let Some(ref enc) = payload_encryptor {
            bg_builder = bg_builder.payload_encryptor(Arc::clone(enc));
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

                // Restore trace context from the first event in the group.
                acteon_server::api::trace_context::restore_trace_context(&group.trace_context);

                info!(
                    group_id = %group.group_id,
                    event_count = group.size(),
                    flushed_at = %event.flushed_at,
                    "group flushed - dispatching notification"
                );

                // Build a summary notification action from the grouped events.
                // Uses the first event's metadata and aggregates the payloads.
                let payloads: Vec<_> = group.events.iter().map(|e| e.payload.clone()).collect();
                let mut summary_payload = serde_json::json!({
                    "group_id": group.group_id,
                    "group_key": group.group_key,
                    "event_count": group.size(),
                    "events": payloads,
                    "labels": group.labels,
                    "flushed_at": event.flushed_at.to_rfc3339(),
                });

                // Mark as a group re-dispatch so quota enforcement is skipped.
                if let Some(obj) = summary_payload.as_object_mut() {
                    obj.insert("_group_dispatch".to_string(), serde_json::Value::Bool(true));
                }

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

                // Emit SSE stream event for the group flush.
                let gw = flush_gateway.read().await;
                let _ = gw.stream_tx().send(StreamEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    timestamp: chrono::Utc::now(),
                    event_type: StreamEventType::GroupFlushed {
                        group_id: group.group_id.clone(),
                        event_count: group.size(),
                    },
                    namespace: namespace.clone(),
                    tenant: tenant.clone(),
                    action_type: None,
                    action_id: None,
                });
                let _ = gw.stream_tx().send(StreamEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    timestamp: chrono::Utc::now(),
                    event_type: StreamEventType::GroupResolved {
                        group_id: group.group_id.clone(),
                        group_key: group.group_key.clone(),
                    },
                    namespace: namespace.clone(),
                    tenant: tenant.clone(),
                    action_type: None,
                    action_id: None,
                });
                drop(gw);

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
                // Restore trace context from the original event that set the timeout.
                acteon_server::api::trace_context::restore_trace_context(&event.trace_context);

                info!(
                    fingerprint = %event.fingerprint,
                    state_machine = %event.state_machine,
                    previous_state = %event.previous_state,
                    new_state = %event.new_state,
                    fired_at = %event.fired_at,
                    "timeout fired - dispatching notification"
                );

                // Emit SSE stream event for the timeout.
                let gw = timeout_gateway.read().await;
                let _ = gw.stream_tx().send(StreamEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    timestamp: chrono::Utc::now(),
                    event_type: StreamEventType::Timeout {
                        fingerprint: event.fingerprint.clone(),
                        state_machine: event.state_machine.clone(),
                        previous_state: event.previous_state.clone(),
                        new_state: event.new_state.clone(),
                    },
                    namespace: timeout_namespace.clone(),
                    tenant: timeout_tenant.clone(),
                    action_type: None,
                    action_id: None,
                });
                drop(gw);

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

        // Spawn consumer for chain advance events.
        // Each advance runs in its own task, bounded by a semaphore.
        let chain_gateway = Arc::clone(&gateway);
        let max_concurrent = config.chains.max_concurrent_advances;
        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
            while let Some(event) = chain_advance_rx.recv().await {
                let permit = Arc::clone(&semaphore).acquire_owned().await;
                let Ok(permit) = permit else {
                    break; // semaphore closed
                };
                let gw = Arc::clone(&chain_gateway);
                tokio::spawn(async move {
                    let _permit = permit;

                    // Load chain state to restore trace context before advancing.
                    let trace_context = {
                        let gw = gw.read().await;
                        gw.get_chain_status(&event.namespace, &event.tenant, &event.chain_id)
                            .await
                            .ok()
                            .flatten()
                            .map(|s| s.origin_action.trace_context)
                    };

                    if let Some(ctx) = trace_context {
                        acteon_server::api::trace_context::restore_trace_context(&ctx);
                    }

                    info!(
                        chain_id = %event.chain_id,
                        namespace = %event.namespace,
                        tenant = %event.tenant,
                        "advancing chain"
                    );

                    // Emit SSE stream event for the chain advance.
                    let gw = gw.read().await;
                    let _ = gw.stream_tx().send(StreamEvent {
                        id: uuid::Uuid::now_v7().to_string(),
                        timestamp: chrono::Utc::now(),
                        event_type: StreamEventType::ChainAdvanced {
                            chain_id: event.chain_id.clone(),
                        },
                        namespace: event.namespace.clone(),
                        tenant: event.tenant.clone(),
                        action_type: None,
                        action_id: None,
                    });

                    if let Err(e) = gw
                        .advance_chain(&event.namespace, &event.tenant, &event.chain_id)
                        .await
                    {
                        tracing::error!(
                            chain_id = %event.chain_id,
                            error = %e,
                            "chain advancement failed"
                        );
                    }
                });
            }
        });

        // Spawn consumer for scheduled action events.
        // Dispatches the action through the gateway and emits an SSE event.
        // The action data key is deleted only after successful dispatch
        // (at-least-once delivery semantics).
        let scheduled_gateway = Arc::clone(&gateway);
        let scheduled_store = Arc::clone(&store);
        tokio::spawn(async move {
            while let Some(event) = scheduled_action_rx.recv().await {
                info!(
                    action_id = %event.action_id,
                    namespace = %event.namespace,
                    tenant = %event.tenant,
                    "dispatching scheduled action"
                );

                // Emit SSE stream event for the scheduled action.
                let gw = scheduled_gateway.read().await;
                let _ = gw.stream_tx().send(StreamEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    timestamp: chrono::Utc::now(),
                    event_type: StreamEventType::ScheduledActionDue {
                        action_id: event.action_id.clone(),
                    },
                    namespace: event.namespace.clone(),
                    tenant: event.tenant.clone(),
                    action_type: Some(event.action.action_type.clone()),
                    action_id: Some(event.action_id.clone()),
                });

                // Mark the action payload so that handle_schedule rejects re-scheduling.
                // This prevents infinite loops when a Schedule rule matches
                // the same action on re-dispatch.
                let mut action = event.action;
                if let Some(obj) = action.payload.as_object_mut() {
                    obj.insert(
                        "_scheduled_dispatch".to_string(),
                        serde_json::Value::Bool(true),
                    );
                }
                match gw.dispatch(action, None).await {
                    Ok(outcome) => {
                        info!(
                            action_id = %event.action_id,
                            ?outcome,
                            "scheduled action dispatched"
                        );
                        // Delete action data only after successful dispatch
                        // (at-least-once delivery). On crash before this point,
                        // the claim key expires and the action is re-delivered.
                        let sched_key = acteon_state::StateKey::new(
                            event.namespace.as_str(),
                            event.tenant.as_str(),
                            acteon_state::KeyKind::ScheduledAction,
                            &event.action_id,
                        );
                        if let Err(e) = scheduled_store.delete(&sched_key).await {
                            tracing::warn!(
                                action_id = %event.action_id,
                                error = %e,
                                "failed to clean up scheduled action data after dispatch"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            action_id = %event.action_id,
                            error = %e,
                            "failed to dispatch scheduled action"
                        );
                    }
                }
            }
        });

        // Spawn consumer for recurring action events.
        // Constructs a concrete Action from the recurring template, dispatches it,
        // updates execution state, and re-indexes the next occurrence.
        let recurring_gateway = Arc::clone(&gateway);
        let recurring_store = Arc::clone(&store);
        tokio::spawn(async move {
            while let Some(event) = recurring_action_rx.recv().await {
                let recurring = &event.recurring_action;
                info!(
                    recurring_id = %event.recurring_id,
                    namespace = %event.namespace,
                    tenant = %event.tenant,
                    cron_expr = %recurring.cron_expr,
                    "processing recurring action"
                );

                // Construct a concrete Action from the template.
                let mut payload = recurring.action_template.payload.clone();
                // Mark as a recurring re-dispatch so quota enforcement
                // does not double-count the action.
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        "_recurring_dispatch".to_string(),
                        serde_json::Value::Bool(true),
                    );
                }
                let action = acteon_core::Action::new(
                    event.namespace.as_str(),
                    event.tenant.as_str(),
                    recurring.action_template.provider.as_str(),
                    recurring.action_template.action_type.as_str(),
                    payload,
                );

                // Dispatch through the gateway.
                let gw = recurring_gateway.read().await;
                let now = chrono::Utc::now();

                match gw.dispatch(action, None).await {
                    Ok(outcome) => {
                        info!(
                            recurring_id = %event.recurring_id,
                            ?outcome,
                            "recurring action dispatched"
                        );
                        gw.metrics().increment_recurring_dispatched();

                        // Update the recurring action state: increment count,
                        // set last_executed_at, compute and index next occurrence.
                        let rec_key = acteon_state::StateKey::new(
                            event.namespace.as_str(),
                            event.tenant.as_str(),
                            acteon_state::KeyKind::RecurringAction,
                            &event.recurring_id,
                        );
                        if let Ok(Some(raw_str)) = recurring_store.get(&rec_key).await
                            && let Ok(data_str) = gw.decrypt_state_value(&raw_str)
                            && let Ok(mut rec) =
                                serde_json::from_str::<acteon_core::RecurringAction>(&data_str)
                        {
                            rec.last_executed_at = Some(now);
                            rec.execution_count += 1;
                            rec.updated_at = now;

                            // Compute next occurrence (no backfill).
                            let next = acteon_core::validate_cron_expr(&rec.cron_expr)
                                .ok()
                                .and_then(|cron| {
                                    acteon_core::validate_timezone(&rec.timezone).ok().and_then(
                                        |tz| acteon_core::next_occurrence(&cron, tz, &now),
                                    )
                                });

                            // Check if the action should still be active.
                            let still_active = rec.enabled
                                && next.is_some()
                                && rec.ends_at.is_none_or(|ends| next.unwrap() <= ends)
                                && rec
                                    .max_executions
                                    .is_none_or(|max| rec.execution_count < max);

                            rec.next_execution_at = if still_active { next } else { None };

                            if let Ok(json) = serde_json::to_string(&rec) {
                                let encrypted = gw.encrypt_state_value(&json).unwrap_or(json);
                                let _ = recurring_store.set(&rec_key, &encrypted, None).await;
                            }

                            // Re-index or remove from pending index.
                            let pending_key = acteon_state::StateKey::new(
                                event.namespace.as_str(),
                                event.tenant.as_str(),
                                acteon_state::KeyKind::PendingRecurring,
                                &event.recurring_id,
                            );

                            if let Some(next_at) = rec.next_execution_at {
                                let next_ms = next_at.timestamp_millis();
                                let _ = recurring_store
                                    .set(&pending_key, &next_ms.to_string(), None)
                                    .await;
                                let _ = recurring_store.index_timeout(&pending_key, next_ms).await;
                            } else {
                                let _ = recurring_store.delete(&pending_key).await;
                                let _ = recurring_store.remove_timeout_index(&pending_key).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            recurring_id = %event.recurring_id,
                            error = %e,
                            "failed to dispatch recurring action"
                        );
                        gw.metrics().increment_recurring_errors();
                    }
                }
            }
        });

        info!("background processor started");
        Some(shutdown_tx)
    } else {
        None
    };

    // Create the per-tenant SSE connection limit registry.
    let connection_registry = Arc::new(acteon_server::api::stream::ConnectionRegistry::new(
        config.server.max_sse_connections_per_tenant.unwrap_or(10),
    ));

    let config_snapshot = ConfigSnapshot::from(&config);

    let state = AppState {
        gateway: Arc::clone(&gateway),
        audit: audit_store,
        auth: auth_provider,
        rate_limiter,
        embedding: embedding_bridge
            .as_ref()
            .map(|b| Arc::clone(b) as Arc<dyn acteon_rules::EmbeddingEvalSupport>),
        embedding_metrics: embedding_bridge.as_ref().map(|b| b.metrics()),
        connection_registry: Some(connection_registry),
        config: config_snapshot,
        ui_path: Some(config.ui.dist_path.clone()),
        ui_enabled: config.ui.enabled,
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

    // Flush pending OpenTelemetry spans before exit.
    telemetry_guard.shutdown();

    info!("acteon-server shut down");
    Ok(())
}

/// Decrypt a config value, requiring `ACTEON_AUTH_KEY` if the value is encrypted.
///
/// - `ENC[...]` values are decrypted using the master key (error if key is missing).
/// - Plain values are returned as-is regardless of whether a key is available.
fn require_decrypt(
    value: &str,
    master_key: Option<&MasterKey>,
) -> Result<String, Box<dyn std::error::Error>> {
    if value.trim().starts_with("ENC[") {
        let mk = master_key.ok_or(
            "ACTEON_AUTH_KEY environment variable is required to decrypt ENC[...] config values",
        )?;
        Ok(decrypt_value(value, mk)?.expose_secret().clone())
    } else {
        Ok(value.to_owned())
    }
}

/// Run the `migrate` subcommand: initialize database schemas for configured backends and exit.
async fn run_migrate(config: &ActeonConfig) -> Result<(), Box<dyn std::error::Error>> {
    info!(backend = %config.state.backend, "running state backend migrations...");
    let (_store, _lock) = acteon_server::state_factory::create_state(&config.state).await?;
    info!(backend = %config.state.backend, "state backend migrations complete");

    if config.audit.enabled {
        info!(backend = %config.audit.backend, "running audit backend migrations...");
        let _audit = acteon_server::audit_factory::create_audit_store(&config.audit).await?;
        info!(backend = %config.audit.backend, "audit backend migrations complete");
    } else {
        info!("audit disabled, skipping audit migrations");
    }

    info!("all migrations complete");
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
