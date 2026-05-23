use super::*;

#[test]
fn telemetry_defaults() {
    let config: TelemetryConfig = toml::from_str("").unwrap();
    assert!(!config.enabled);
    assert_eq!(config.endpoint, "http://localhost:4317");
    assert_eq!(config.service_name, "acteon");
    assert!((config.sample_ratio - 1.0).abs() < f64::EPSILON);
    assert_eq!(config.protocol, "grpc");
    assert_eq!(config.timeout_seconds, 10);
    assert!(config.resource_attributes.is_empty());
}

#[test]
fn telemetry_custom_config() {
    let toml = r#"
        enabled = true
        endpoint = "http://collector:4317"
        service_name = "my-acteon"
        sample_ratio = 0.5
        protocol = "http"
        timeout_seconds = 30

        [resource_attributes]
        "deployment.environment" = "staging"
        "host.name" = "node-1"
    "#;

    let config: TelemetryConfig = toml::from_str(toml).unwrap();
    assert!(config.enabled);
    assert_eq!(config.endpoint, "http://collector:4317");
    assert_eq!(config.service_name, "my-acteon");
    assert!((config.sample_ratio - 0.5).abs() < f64::EPSILON);
    assert_eq!(config.protocol, "http");
    assert_eq!(config.timeout_seconds, 30);
    assert_eq!(config.resource_attributes.len(), 2);
    assert_eq!(
        config
            .resource_attributes
            .get("deployment.environment")
            .unwrap(),
        "staging"
    );
    assert_eq!(
        config.resource_attributes.get("host.name").unwrap(),
        "node-1"
    );
}

#[test]
fn telemetry_disabled() {
    let toml = r#"
        enabled = false
    "#;

    let config: TelemetryConfig = toml::from_str(toml).unwrap();
    assert!(!config.enabled);
    // All other fields should still get defaults
    assert_eq!(config.endpoint, "http://localhost:4317");
    assert_eq!(config.service_name, "acteon");
}

#[test]
fn telemetry_sample_ratio_bounds() {
    // Ratio = 0.0 (no sampling)
    let config: TelemetryConfig = toml::from_str("sample_ratio = 0.0").unwrap();
    assert!(config.sample_ratio <= 0.0);

    // Ratio = 0.5 (50% sampling)
    let config: TelemetryConfig = toml::from_str("sample_ratio = 0.5").unwrap();
    assert!((config.sample_ratio - 0.5).abs() < f64::EPSILON);

    // Ratio = 1.0 (100% sampling — default)
    let config: TelemetryConfig = toml::from_str("sample_ratio = 1.0").unwrap();
    assert!((config.sample_ratio - 1.0).abs() < f64::EPSILON);
}

#[test]
fn telemetry_protocol_grpc() {
    let config: TelemetryConfig = toml::from_str(r#"protocol = "grpc""#).unwrap();
    assert_eq!(config.protocol, "grpc");
}

#[test]
fn telemetry_protocol_http() {
    let config: TelemetryConfig = toml::from_str(r#"protocol = "http""#).unwrap();
    assert_eq!(config.protocol, "http");
}

#[test]
fn telemetry_empty_resource_attributes() {
    let config: TelemetryConfig = toml::from_str("[resource_attributes]").unwrap();
    assert!(config.resource_attributes.is_empty());
}

#[test]
fn telemetry_in_acteon_config() {
    let toml = r#"
        [telemetry]
        enabled = true
        endpoint = "http://tempo:4317"
        sample_ratio = 0.1
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert!(config.telemetry.enabled);
    assert_eq!(config.telemetry.endpoint, "http://tempo:4317");
    assert!((config.telemetry.sample_ratio - 0.1).abs() < f64::EPSILON);
}

#[test]
fn telemetry_absent_from_acteon_config_uses_defaults() {
    let config: ActeonConfig = toml::from_str("").unwrap();
    assert!(!config.telemetry.enabled);
    assert_eq!(config.telemetry.endpoint, "http://localhost:4317");
}

#[test]
fn providers_default_empty() {
    let config: ActeonConfig = toml::from_str("").unwrap();
    assert!(config.providers.is_empty());
}

#[test]
fn providers_parsed_from_toml() {
    let toml = r#"
        [[providers]]
        name = "email"
        type = "webhook"
        url = "http://localhost:9999/webhook"

        [providers.headers]
        Authorization = "Bearer token"

        [[providers]]
        name = "slack"
        type = "log"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.providers.len(), 2);

    assert_eq!(config.providers[0].name, "email");
    assert_eq!(config.providers[0].provider_type, "webhook");
    assert_eq!(
        config.providers[0].url.as_deref(),
        Some("http://localhost:9999/webhook")
    );
    assert_eq!(
        config.providers[0].headers.get("Authorization").unwrap(),
        "Bearer token"
    );

    assert_eq!(config.providers[1].name, "slack");
    assert_eq!(config.providers[1].provider_type, "log");
    assert!(config.providers[1].url.is_none());
    assert!(config.providers[1].headers.is_empty());
}

#[test]
fn config_snapshot_masks_secrets() {
    let toml = r#"
        [server]
        host = "0.0.0.0"
        port = 9090
        approval_secret = "deadbeef"

        [[server.approval_keys]]
        id = "k1"
        secret = "cafebabe"

        [llm_guardrail]
        enabled = true
        api_key = "sk-secret-key-value"
        policy = "You are a safety checker for actions."

        [embedding]
        enabled = true
        api_key = "sk-embed-key"

        [[providers]]
        name = "email"
        type = "webhook"
        url = "http://localhost:9999/webhook"

        [providers.headers]
        Authorization = "Bearer secret-token"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let snapshot = ConfigSnapshot::from(&config);

    // Server: secrets not present in snapshot
    assert_eq!(snapshot.server.host, "0.0.0.0");
    assert_eq!(snapshot.server.port, 9090);

    // LLM guardrail: api_key masked as boolean
    assert!(snapshot.llm_guardrail.has_api_key);
    assert_eq!(
        snapshot.llm_guardrail.policy,
        "You are a safety checker for actions."
    );

    // UI: enabled and dist_path present
    assert!(snapshot.ui.enabled);
    assert_eq!(snapshot.ui.dist_path, "ui/dist");

    // Embedding: api_key masked as boolean
    assert!(snapshot.embedding.has_api_key);

    // Provider: headers hidden, only count shown
    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].name, "email");
    assert_eq!(snapshot.providers[0].header_count, 1);
}

#[test]
fn config_snapshot_truncates_long_policy() {
    let long_policy = "x".repeat(200);
    let toml_str = format!(
        r#"
        [llm_guardrail]
        policy = "{long_policy}"
    "#
    );

    let config: ActeonConfig = toml::from_str(&toml_str).unwrap();
    let snapshot = ConfigSnapshot::from(&config);

    assert_eq!(snapshot.llm_guardrail.policy.len(), 103); // 100 chars + "..."
    assert!(snapshot.llm_guardrail.policy.ends_with("..."));
}

#[test]
fn config_snapshot_empty_api_key_reports_false() {
    let config: ActeonConfig = toml::from_str("").unwrap();
    let snapshot = ConfigSnapshot::from(&config);

    assert!(!snapshot.llm_guardrail.has_api_key);
    assert!(!snapshot.embedding.has_api_key);
}

#[test]
fn twilio_provider_parsed_from_toml() {
    let toml = r#"
        [[providers]]
        name = "sms"
        type = "twilio"
        account_sid = "ACXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
        auth_token = "test-placeholder-token"
        from_number = "+15551234567"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].name, "sms");
    assert_eq!(config.providers[0].provider_type, "twilio");
    assert_eq!(
        config.providers[0].account_sid.as_deref(),
        Some("ACXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
    );
    assert_eq!(
        config.providers[0].auth_token.as_deref(),
        Some("test-placeholder-token")
    );
    assert_eq!(
        config.providers[0].from_number.as_deref(),
        Some("+15551234567")
    );

    let snapshot = ProviderSnapshot::from(&config.providers[0]);
    assert!(snapshot.has_auth_token);
    assert!(!snapshot.has_webhook_url);
    assert!(!snapshot.has_token);
}

#[test]
fn teams_provider_parsed_from_toml() {
    let toml = r#"
        [[providers]]
        name = "teams-alerts"
        type = "teams"
        webhook_url = "https://outlook.office.com/webhook/test"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].provider_type, "teams");
    assert_eq!(
        config.providers[0].webhook_url.as_deref(),
        Some("https://outlook.office.com/webhook/test")
    );

    let snapshot = ProviderSnapshot::from(&config.providers[0]);
    assert!(snapshot.has_webhook_url);
    assert!(!snapshot.has_auth_token);
}

#[test]
fn discord_provider_parsed_from_toml() {
    let toml = r#"
        [[providers]]
        name = "discord-alerts"
        type = "discord"
        webhook_url = "https://discord.com/api/webhooks/123/abc"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].provider_type, "discord");
    assert_eq!(
        config.providers[0].webhook_url.as_deref(),
        Some("https://discord.com/api/webhooks/123/abc")
    );

    let snapshot = ProviderSnapshot::from(&config.providers[0]);
    assert!(snapshot.has_webhook_url);
}

#[test]
fn config_snapshot_serializes_to_json() {
    let config: ActeonConfig = toml::from_str("").unwrap();
    let snapshot = ConfigSnapshot::from(&config);

    let json = serde_json::to_value(&snapshot).unwrap();
    assert!(json.is_object());
    assert!(json.get("server").is_some());
    assert!(json.get("llm_guardrail").is_some());
    assert!(json.get("providers").is_some());
}

#[test]
fn background_config_defaults() {
    let config: ActeonConfig = toml::from_str("").unwrap();
    assert!(!config.background.enable_recurring_actions);
    assert_eq!(config.background.recurring_check_interval_seconds, 60);
    assert!(!config.background.enable_scheduled_actions);
    assert_eq!(config.background.scheduled_check_interval_seconds, 5);
}

#[test]
fn background_config_recurring_enabled() {
    let toml = r#"
        [background]
        enable_recurring_actions = true
        recurring_check_interval_seconds = 30
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert!(config.background.enable_recurring_actions);
    assert_eq!(config.background.recurring_check_interval_seconds, 30);
}

#[test]
fn background_snapshot_includes_recurring_fields() {
    let toml = r#"
        [background]
        enable_recurring_actions = true
        recurring_check_interval_seconds = 120
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let snapshot = ConfigSnapshot::from(&config);
    assert!(snapshot.background.enable_recurring_actions);
    assert_eq!(snapshot.background.recurring_check_interval_seconds, 120);
}

#[test]
fn background_snapshot_recurring_defaults() {
    let config: ActeonConfig = toml::from_str("").unwrap();
    let snapshot = ConfigSnapshot::from(&config);
    assert!(!snapshot.background.enable_recurring_actions);
    assert_eq!(snapshot.background.recurring_check_interval_seconds, 60);
}

#[test]
fn config_snapshot_chains_summary() {
    let toml = r#"
        [chains]
        max_concurrent_advances = 8
        completed_chain_ttl_seconds = 3600

        [[chains.definitions]]
        name = "onboarding"
        timeout_seconds = 300

        [[chains.definitions.steps]]
        name = "step1"
        provider = "email"
        action_type = "send_welcome"
        payload_template = {}

        [[chains.definitions.steps]]
        name = "step2"
        provider = "slack"
        action_type = "notify"
        payload_template = {}
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let snapshot = ConfigSnapshot::from(&config);

    assert_eq!(snapshot.chains.max_concurrent_advances, 8);
    assert_eq!(snapshot.chains.completed_chain_ttl_seconds, 3600);
    assert_eq!(snapshot.chains.definitions.len(), 1);
    assert_eq!(snapshot.chains.definitions[0].name, "onboarding");
    assert_eq!(snapshot.chains.definitions[0].steps_count, 2);
    assert_eq!(snapshot.chains.definitions[0].timeout_seconds, Some(300));
}

#[test]
fn wasm_config_defaults() {
    let config = WasmServerConfig::default();
    assert!(!config.enabled);
    assert!(config.plugin_dir.is_none());
    assert_eq!(config.default_memory_limit_bytes, 16 * 1024 * 1024);
    assert_eq!(config.default_timeout_ms, 100);
}

#[test]
fn wasm_config_from_toml() {
    let toml = r#"
        [wasm]
        enabled = true
        plugin_dir = "/etc/acteon/plugins"
        default_memory_limit_bytes = 33554432
        default_timeout_ms = 200
    "#;
    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert!(config.wasm.enabled);
    assert_eq!(
        config.wasm.plugin_dir.as_deref(),
        Some("/etc/acteon/plugins")
    );
    assert_eq!(config.wasm.default_memory_limit_bytes, 33_554_432);
    assert_eq!(config.wasm.default_timeout_ms, 200);
}

#[test]
fn wasm_config_omitted_uses_defaults() {
    let toml = "";
    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert!(!config.wasm.enabled);
    assert!(config.wasm.plugin_dir.is_none());
    assert_eq!(config.wasm.default_memory_limit_bytes, 16 * 1024 * 1024);
    assert_eq!(config.wasm.default_timeout_ms, 100);
}

#[test]
fn wasm_snapshot_from_config() {
    let toml = r#"
        [wasm]
        enabled = true
        plugin_dir = "/plugins"
    "#;
    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let snapshot = ConfigSnapshot::from(&config);
    assert!(snapshot.wasm.enabled);
    assert_eq!(snapshot.wasm.plugin_dir.as_deref(), Some("/plugins"));
    assert_eq!(snapshot.wasm.default_memory_limit_bytes, 16 * 1024 * 1024);
}

#[test]
fn chain_step_sub_chain_parsed_from_toml() {
    let toml = r#"
        [[chains.definitions]]
        name = "parent-chain"

        [[chains.definitions.steps]]
        name = "step1"
        provider = "email"
        action_type = "send_welcome"
        payload_template = {}

        [[chains.definitions.steps]]
        name = "invoke-notify"
        sub_chain = "notify-chain"

        [[chains.definitions.steps]]
        name = "step3"
        provider = "slack"
        action_type = "confirm"
        payload_template = {}
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.chains.definitions.len(), 1);
    let chain = &config.chains.definitions[0];
    assert_eq!(chain.steps.len(), 3);

    // Regular step
    assert_eq!(chain.steps[0].provider.as_deref(), Some("email"));
    assert_eq!(chain.steps[0].action_type.as_deref(), Some("send_welcome"));
    assert!(chain.steps[0].sub_chain.is_none());

    // Sub-chain step
    assert!(chain.steps[1].provider.is_none());
    assert!(chain.steps[1].action_type.is_none());
    assert_eq!(chain.steps[1].sub_chain.as_deref(), Some("notify-chain"));

    // Regular step after sub-chain
    assert_eq!(chain.steps[2].provider.as_deref(), Some("slack"));
    assert!(chain.steps[2].sub_chain.is_none());
}

#[test]
fn chain_step_sub_chain_with_delay_and_on_failure() {
    let toml = r#"
        [[chains.definitions]]
        name = "with-options"

        [[chains.definitions.steps]]
        name = "invoke-child"
        sub_chain = "child-chain"
        delay_seconds = 30
        on_failure = "skip"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let step = &config.chains.definitions[0].steps[0];
    assert_eq!(step.sub_chain.as_deref(), Some("child-chain"));
    assert_eq!(step.delay_seconds, Some(30));
    assert_eq!(step.on_failure.as_deref(), Some("skip"));
}

#[test]
fn chain_step_backward_compat_no_sub_chain() {
    // Existing TOML configs without sub_chain should still parse correctly.
    let toml = r#"
        [[chains.definitions]]
        name = "legacy"

        [[chains.definitions.steps]]
        name = "step1"
        provider = "email"
        action_type = "send"

        [chains.definitions.steps.payload_template]
        msg = "hello"
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let step = &config.chains.definitions[0].steps[0];
    assert_eq!(step.provider.as_deref(), Some("email"));
    assert_eq!(step.action_type.as_deref(), Some("send"));
    assert!(step.sub_chain.is_none());
}

#[test]
fn enrichment_config_parses_with_defaults() {
    let toml = r#"
        [[enrichments]]
        name = "fetch-asg"
        lookup_provider = "cost-asg"
        resource_type = "auto_scaling_group"
        merge_key = "asg_data"

        [enrichments.params]
        auto_scaling_group_names = ["my-asg"]
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.enrichments.len(), 1);
    let e = &config.enrichments[0];
    assert_eq!(e.name, "fetch-asg");
    assert_eq!(e.lookup_provider, "cost-asg");
    assert_eq!(e.resource_type, "auto_scaling_group");
    assert_eq!(e.merge_key, "asg_data");
    assert_eq!(e.timeout_seconds, 5); // default
    assert_eq!(
        e.failure_policy,
        acteon_core::EnrichmentFailurePolicy::FailOpen
    ); // default
    assert!(e.namespace.is_none());
    assert!(e.tenant.is_none());
    assert!(e.action_type.is_none());
    assert!(e.provider.is_none());
}

#[test]
fn enrichment_config_parses_full() {
    let toml = r#"
        [[enrichments]]
        name = "fetch-asg-state"
        namespace = "infra"
        tenant = "prod"
        action_type = "terminate_instances"
        provider = "cost-ec2"
        lookup_provider = "cost-asg"
        resource_type = "auto_scaling_group"
        merge_key = "current_asg_state"
        timeout_seconds = 10
        failure_policy = "fail_closed"

        [enrichments.params]
        auto_scaling_group_names = ["{{payload.asg_name}}"]
    "#;

    let config: ActeonConfig = toml::from_str(toml).unwrap();
    let e = &config.enrichments[0];
    assert_eq!(e.namespace.as_deref(), Some("infra"));
    assert_eq!(e.tenant.as_deref(), Some("prod"));
    assert_eq!(e.action_type.as_deref(), Some("terminate_instances"));
    assert_eq!(e.provider.as_deref(), Some("cost-ec2"));
    assert_eq!(e.timeout_seconds, 10);
    assert_eq!(
        e.failure_policy,
        acteon_core::EnrichmentFailurePolicy::FailClosed
    );
}
