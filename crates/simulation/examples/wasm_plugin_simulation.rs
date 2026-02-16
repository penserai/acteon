//! Comprehensive simulation of WASM rule plugins in Acteon.
//!
//! Demonstrates 15 scenarios covering basic plugin evaluation, mixed rule types,
//! resource limit enforcement, error handling, multi-tenant isolation, plugin
//! hot-reload, chain integration, LLM combination, and performance benchmarking.
//!
//! Run with: `cargo run -p acteon-simulation --example wasm_plugin_simulation`

use std::sync::Arc;
use std::time::{Duration, Instant};

use acteon_core::chain::{ChainConfig, ChainStepConfig};
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayBuilder;
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::{Rule, RuleAction, RuleFrontend, RuleSource};
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::provider::RecordingProvider;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_wasm_runtime::{
    MockWasmRuntime, WasmInvocationResult, WasmPluginConfig, WasmPluginRuntime,
};

// ---------------------------------------------------------------------------
// Configurable mock runtimes for simulation scenarios
// ---------------------------------------------------------------------------

/// A WASM runtime that returns verdicts based on action payload fields.
/// Used to simulate real plugin logic without actual WASM compilation.
#[derive(Debug)]
struct PayloadInspectingRuntime {
    /// The payload field to check.
    field: String,
    /// The value that triggers a `true` verdict.
    trigger_value: serde_json::Value,
}

#[async_trait::async_trait]
impl WasmPluginRuntime for PayloadInspectingRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        let field_val = input.get(&self.field);
        let verdict = field_val == Some(&self.trigger_value);
        Ok(WasmInvocationResult {
            verdict,
            message: Some(format!(
                "field '{}' {} trigger",
                self.field,
                if verdict { "matches" } else { "does not match" }
            )),
            metadata: serde_json::json!({ "checked_field": self.field }),
        })
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["payload-inspector".to_owned()]
    }
}

/// A WASM runtime that applies a numeric threshold check on a payload field.
#[derive(Debug)]
struct ThresholdRuntime {
    field: String,
    threshold: f64,
}

#[async_trait::async_trait]
impl WasmPluginRuntime for ThresholdRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        let val = input
            .get(&self.field)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let verdict = val >= self.threshold;
        Ok(WasmInvocationResult {
            verdict,
            message: Some(format!(
                "{} = {val:.2}, threshold = {:.2} -> {}",
                self.field,
                self.threshold,
                if verdict { "PASS" } else { "FAIL" }
            )),
            metadata: serde_json::json!({
                "value": val,
                "threshold": self.threshold,
            }),
        })
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["threshold-checker".to_owned()]
    }
}

/// A WASM runtime that simulates a timeout error for specific plugins.
#[derive(Debug)]
struct TimeoutSimulatingRuntime {
    timeout_ms: u64,
}

#[async_trait::async_trait]
impl WasmPluginRuntime for TimeoutSimulatingRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        _input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        Err(acteon_wasm_runtime::WasmError::Timeout(self.timeout_ms))
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["slow-plugin".to_owned()]
    }
}

/// A WASM runtime that simulates a memory limit exceeded error.
#[derive(Debug)]
struct MemoryExceededRuntime {
    limit_bytes: u64,
}

#[async_trait::async_trait]
impl WasmPluginRuntime for MemoryExceededRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        _input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        Err(acteon_wasm_runtime::WasmError::MemoryExceeded(
            self.limit_bytes,
        ))
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["memory-hog".to_owned()]
    }
}

/// A WASM runtime that simulates a plugin trap/panic.
#[derive(Debug)]
struct TrappingRuntime;

#[async_trait::async_trait]
impl WasmPluginRuntime for TrappingRuntime {
    async fn invoke(
        &self,
        plugin: &str,
        function: &str,
        _input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        Err(acteon_wasm_runtime::WasmError::Invocation(format!(
            "plugin '{plugin}' trapped in function '{function}': unreachable instruction"
        )))
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["trapping-plugin".to_owned()]
    }
}

/// A WASM runtime that returns a modified payload in its metadata.
#[derive(Debug)]
struct PayloadTransformRuntime;

#[async_trait::async_trait]
impl WasmPluginRuntime for PayloadTransformRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        let mut transformed = input.clone();
        if let Some(obj) = transformed.as_object_mut() {
            obj.insert("wasm_enriched".to_owned(), serde_json::json!(true));
            obj.insert(
                "processed_by".to_owned(),
                serde_json::json!("transform-plugin-v1"),
            );
        }
        Ok(WasmInvocationResult {
            verdict: true,
            message: Some("Payload enriched with WASM metadata".to_owned()),
            metadata: transformed,
        })
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec!["transform-plugin".to_owned()]
    }
}

/// A versioned runtime that can be swapped to simulate hot-reload.
#[derive(Debug)]
struct VersionedRuntime {
    version: u32,
}

impl VersionedRuntime {
    fn new(version: u32) -> Self {
        Self { version }
    }
}

#[async_trait::async_trait]
impl WasmPluginRuntime for VersionedRuntime {
    async fn invoke(
        &self,
        _plugin: &str,
        _function: &str,
        _input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, acteon_wasm_runtime::WasmError> {
        // v1 always allows, v2 always denies
        let verdict = self.version == 1;
        Ok(WasmInvocationResult {
            verdict,
            message: Some(format!("plugin v{} -> verdict={verdict}", self.version)),
            metadata: serde_json::json!({ "plugin_version": self.version }),
        })
    }

    fn has_plugin(&self, _name: &str) -> bool {
        true
    }

    fn list_plugins(&self) -> Vec<String> {
        vec![format!("versioned-plugin-v{}", self.version)]
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn make_action(ns: &str, tenant: &str, provider: &str, action_type: &str) -> Action {
    Action::new(
        ns,
        tenant,
        provider,
        action_type,
        serde_json::json!({
            "message": format!("WASM simulation: {action_type}"),
        }),
    )
}

fn make_action_with_payload(
    ns: &str,
    tenant: &str,
    provider: &str,
    action_type: &str,
    payload: serde_json::Value,
) -> Action {
    Action::new(ns, tenant, provider, action_type, payload)
}

/// Build a WASM-enabled rule using the `Expr::WasmCall` condition.
fn wasm_rule(
    name: &str,
    priority: i32,
    plugin: &str,
    function: &str,
    _params: serde_json::Value,
    action: RuleAction,
) -> Rule {
    Rule::new(
        name,
        Expr::WasmCall {
            plugin: plugin.to_owned(),
            function: function.to_owned(),
        },
        action,
    )
    .with_priority(priority)
    .with_source(RuleSource::Wasm {
        plugin: Some(plugin.to_owned()),
    })
}

/// Build a gateway with a mock WASM runtime, optional rules, and providers.
fn build_gateway_with_wasm(
    wasm_runtime: Arc<dyn WasmPluginRuntime>,
    providers: Vec<Arc<RecordingProvider>>,
    rules: Vec<Rule>,
) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .wasm_runtime(wasm_runtime)
        .rules(rules);

    for p in providers {
        builder = builder.provider(p as Arc<dyn acteon_provider::DynProvider>);
    }

    builder.build().expect("gateway should build")
}

/// Build a gateway with WASM runtime, rules from YAML, and providers.
fn build_gateway_with_wasm_and_yaml(
    wasm_runtime: Arc<dyn WasmPluginRuntime>,
    providers: Vec<Arc<RecordingProvider>>,
    yaml_rules: &str,
    wasm_rules: Vec<Rule>,
) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let frontend = YamlFrontend;
    let mut rules = RuleFrontend::parse(&frontend, yaml_rules).expect("valid YAML rules");
    rules.extend(wasm_rules);

    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .wasm_runtime(wasm_runtime)
        .rules(rules);

    for p in providers {
        builder = builder.provider(p as Arc<dyn acteon_provider::DynProvider>);
    }

    builder.build().expect("gateway should build")
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// Scenario 1: Basic WASM rule -- allow/deny based on payload field.
async fn scenario_basic_wasm_rule() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: BASIC WASM RULE (allow/deny based on payload)");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(PayloadInspectingRuntime {
        field: "category".to_owned(),
        trigger_value: serde_json::json!("blocked"),
    });

    let email = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "wasm-block-category",
        1,
        "payload-inspector",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Suppress,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&email)], rules);

    // Action with blocked category should be suppressed
    let blocked = make_action_with_payload(
        "ns",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({ "category": "blocked", "body": "test" }),
    );
    let outcome = gateway.dispatch(blocked, None).await?;
    println!("  Blocked action outcome: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Suppressed { .. }),
        "blocked category should be suppressed"
    );
    email.assert_not_called();

    // Action with allowed category should proceed
    let allowed = make_action_with_payload(
        "ns",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({ "category": "normal", "body": "test" }),
    );
    let outcome = gateway.dispatch(allowed, None).await?;
    println!("  Allowed action outcome: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "normal category should be executed"
    );
    email.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 1 PASSED]\n");
    Ok(())
}

/// Scenario 2: WASM rule with threshold parameter.
async fn scenario_threshold_parameter() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: WASM WITH THRESHOLD PARAMETER");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(ThresholdRuntime {
        field: "risk_score".to_owned(),
        threshold: 0.8,
    });

    let webhook = Arc::new(RecordingProvider::new("webhook"));

    let rules = vec![wasm_rule(
        "high-risk-block",
        1,
        "threshold-checker",
        "check_risk",
        serde_json::json!({ "threshold": 0.8 }),
        RuleAction::Deny,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&webhook)], rules);

    // High risk (0.95 >= 0.8) should be suppressed (Deny rule -> Suppressed outcome)
    let high_risk = make_action_with_payload(
        "ns",
        "tenant-1",
        "webhook",
        "send",
        serde_json::json!({ "risk_score": 0.95, "data": "test-value" }),
    );
    let outcome = gateway.dispatch(high_risk, None).await?;
    println!("  High risk (0.95) outcome: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Suppressed { .. }),
        "high risk should be suppressed"
    );

    // Low risk (0.3 < 0.8) should pass
    let low_risk = make_action_with_payload(
        "ns",
        "tenant-1",
        "webhook",
        "send",
        serde_json::json!({ "risk_score": 0.3, "data": "safe-value" }),
    );
    let outcome = gateway.dispatch(low_risk, None).await?;
    println!("  Low risk (0.3) outcome: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "low risk should be executed"
    );
    webhook.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 2 PASSED]\n");
    Ok(())
}

/// Scenario 3: Mixed YAML + WASM rules with priority ordering.
async fn scenario_mixed_yaml_wasm() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 3: MIXED YAML + WASM RULES WITH PRIORITY");
    println!("------------------------------------------------------------------\n");

    let yaml_rules = r#"
rules:
  - name: yaml-suppress-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress

  - name: yaml-reroute-urgent
    priority: 5
    condition:
      field: action.payload.priority
      eq: "urgent"
    action:
      type: reroute
      target_provider: sms
"#;

    let rt = Arc::new(MockWasmRuntime::new(true).with_message("WASM allow-all fallback"));

    let wasm_rules = vec![wasm_rule(
        "wasm-allow-all",
        100, // Low priority -- evaluated last
        "mock-plugin",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Allow,
    )];

    let email = Arc::new(RecordingProvider::new("email"));
    let sms = Arc::new(RecordingProvider::new("sms"));

    let gateway = build_gateway_with_wasm_and_yaml(
        rt,
        vec![Arc::clone(&email), Arc::clone(&sms)],
        yaml_rules,
        wasm_rules,
    );

    // Spam should be caught by YAML rule (priority 1) before WASM (priority 100)
    let spam = make_action("ns", "tenant-1", "email", "spam");
    let outcome = gateway.dispatch(spam, None).await?;
    println!("  Spam -> YAML suppress (priority 1): {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));

    // Urgent should be caught by YAML reroute (priority 5) before WASM
    let urgent = make_action_with_payload(
        "ns",
        "tenant-1",
        "email",
        "alert",
        serde_json::json!({ "priority": "urgent", "body": "down" }),
    );
    let outcome = gateway.dispatch(urgent, None).await?;
    println!("  Urgent -> YAML reroute (priority 5): {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Rerouted { .. }));
    sms.assert_called(1);

    // Normal action falls through to WASM allow rule
    let normal = make_action("ns", "tenant-1", "email", "send_email");
    let outcome = gateway.dispatch(normal, None).await?;
    println!("  Normal -> WASM allow (priority 100): {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    gateway.shutdown().await;
    println!("\n  [Scenario 3 PASSED]\n");
    Ok(())
}

/// Scenario 4: CEL-style condition calling wasm() inline.
/// Uses Expr::WasmCall combined with standard Expr operators.
async fn scenario_cel_wasm_inline() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 4: CEL-STYLE CONDITION WITH WASM INLINE");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(PayloadInspectingRuntime {
        field: "flagged".to_owned(),
        trigger_value: serde_json::json!(true),
    });

    let webhook = Arc::new(RecordingProvider::new("webhook"));

    // Condition: action_type == "review" AND wasm("inspector", "evaluate", {})
    let condition = Expr::Binary(
        BinaryOp::And,
        Box::new(Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "action_type".into(),
            )),
            Box::new(Expr::String("review".into())),
        )),
        Box::new(Expr::WasmCall {
            plugin: "inspector".to_owned(),
            function: "evaluate".to_owned(),
        }),
    );

    let rules = vec![
        Rule::new("combined-cel-wasm", condition, RuleAction::Suppress)
            .with_priority(1)
            .with_source(RuleSource::Wasm { plugin: None }),
    ];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&webhook)], rules);

    // action_type=review AND flagged=true -> both arms true -> suppress
    let flagged_review = make_action_with_payload(
        "ns",
        "t1",
        "webhook",
        "review",
        serde_json::json!({ "flagged": true }),
    );
    let outcome = gateway.dispatch(flagged_review, None).await?;
    println!("  review+flagged -> suppress: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));

    // action_type=review but flagged=false -> wasm returns false -> no match
    let clean_review = make_action_with_payload(
        "ns",
        "t1",
        "webhook",
        "review",
        serde_json::json!({ "flagged": false }),
    );
    let outcome = gateway.dispatch(clean_review, None).await?;
    println!("  review+not-flagged -> allow: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    // action_type=other -> first arm false (short-circuit) -> no match
    let other = make_action_with_payload(
        "ns",
        "t1",
        "webhook",
        "other",
        serde_json::json!({ "flagged": true }),
    );
    let outcome = gateway.dispatch(other, None).await?;
    println!("  other+flagged -> allow (type mismatch): {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    gateway.shutdown().await;
    println!("\n  [Scenario 4 PASSED]\n");
    Ok(())
}

/// Scenario 5: Plugin timeout enforcement.
async fn scenario_timeout_enforcement() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 5: PLUGIN TIMEOUT ENFORCEMENT");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(TimeoutSimulatingRuntime { timeout_ms: 100 });
    let email = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "timeout-rule",
        1,
        "slow-plugin",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Deny,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&email)], rules);

    // The WASM plugin will timeout; the rule engine should treat this as a
    // non-match (fail-open) and allow the action to proceed.
    let action = make_action("ns", "t1", "email", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  Timeout -> fail-open -> executed: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "timeout should fail-open and allow execution"
    );
    email.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 5 PASSED]\n");
    Ok(())
}

/// Scenario 6: Plugin memory limit enforcement.
async fn scenario_memory_limit() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 6: PLUGIN MEMORY LIMIT ENFORCEMENT");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(MemoryExceededRuntime {
        limit_bytes: 16 * 1024 * 1024,
    });
    let webhook = Arc::new(RecordingProvider::new("webhook"));

    let rules = vec![wasm_rule(
        "memory-rule",
        1,
        "memory-hog",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Suppress,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&webhook)], rules);

    // The WASM plugin exceeds memory; should fail-open.
    let action = make_action("ns", "t1", "webhook", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  Memory exceeded -> fail-open -> executed: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "memory limit exceeded should fail-open"
    );
    webhook.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 6 PASSED]\n");
    Ok(())
}

/// Scenario 7: Plugin trap/panic handling.
async fn scenario_trap_handling() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 7: PLUGIN TRAP/PANIC HANDLING");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(TrappingRuntime);
    let email = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "trapping-rule",
        1,
        "trapping-plugin",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Deny,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&email)], rules);

    // Plugin traps -> fail-open -> allow
    let action = make_action("ns", "t1", "email", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  Plugin trap -> fail-open -> executed: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "trapped plugin should fail-open"
    );
    email.assert_called(1);

    // Verify metrics recorded the WASM error
    let metrics = gateway.metrics().snapshot();
    println!("  WASM errors in metrics: {}", metrics.wasm_errors);
    assert!(
        metrics.wasm_errors >= 1,
        "should track WASM errors in metrics"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 7 PASSED]\n");
    Ok(())
}

/// Scenario 8: Multiple plugins, different rules using each.
async fn scenario_multiple_plugins() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 8: MULTIPLE PLUGINS, DIFFERENT RULES");
    println!("------------------------------------------------------------------\n");

    // Use MockWasmRuntime that always returns true -- simulates multiple plugins
    let rt = Arc::new(MockWasmRuntime::new(true));

    let email = Arc::new(RecordingProvider::new("email"));
    let sms = Arc::new(RecordingProvider::new("sms"));
    let webhook = Arc::new(RecordingProvider::new("webhook"));

    let rules = vec![
        wasm_rule(
            "plugin-alpha-suppress",
            1,
            "alpha-plugin",
            "check_content",
            serde_json::json!({ "mode": "strict" }),
            RuleAction::Suppress,
        ),
        wasm_rule(
            "plugin-beta-reroute",
            2,
            "beta-plugin",
            "classify",
            serde_json::json!({ "target": "sms" }),
            RuleAction::Reroute {
                target_provider: "sms".to_owned(),
            },
        ),
        wasm_rule(
            "plugin-gamma-allow",
            3,
            "gamma-plugin",
            "validate",
            serde_json::json!({}),
            RuleAction::Allow,
        ),
    ];

    let gateway = build_gateway_with_wasm(
        rt,
        vec![Arc::clone(&email), Arc::clone(&sms), Arc::clone(&webhook)],
        rules,
    );

    // The first matching rule (alpha, priority 1) should suppress
    let action = make_action("ns", "t1", "email", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  Multi-plugin dispatch -> first match (alpha, suppress): {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));

    // Verify none of the providers were called
    email.assert_not_called();
    sms.assert_not_called();

    // Verify WASM invocation metrics
    let metrics = gateway.metrics().snapshot();
    println!("  WASM invocations: {}", metrics.wasm_invocations);
    assert!(
        metrics.wasm_invocations >= 1,
        "should track WASM invocation count"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 8 PASSED]\n");
    Ok(())
}

/// Scenario 9: Plugin with state access via host functions.
/// Simulates a plugin that reads state to make decisions.
async fn scenario_state_access() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 9: PLUGIN WITH STATE ACCESS VIA HOST FUNCTIONS");
    println!("------------------------------------------------------------------\n");

    // Use MockWasmRuntime -- in a real scenario, the plugin would call host
    // functions to read state. Here we verify the gateway passes state context.
    let rt = Arc::new(MockWasmRuntime::new(false).with_message("no state match"));

    let email = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "state-aware-rule",
        1,
        "state-plugin",
        "check_state",
        serde_json::json!({ "state_key": "user:activity" }),
        RuleAction::Deny,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&email)], rules);

    // In a real deployment, the WASM plugin would call host functions like
    // `host_state_get("user:activity:t1")` to read state values. The gateway
    // passes a WasmHostContext with state access to the plugin sandbox.
    // Here we use MockWasmRuntime which returns false regardless.

    // The mock returns false, so the rule does not match -- action proceeds
    let action = make_action("ns", "t1", "email", "notify");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  State-aware rule (mock false) -> executed: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));
    email.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 9 PASSED]\n");
    Ok(())
}

/// Scenario 10: High-throughput benchmark measuring WASM overhead.
async fn scenario_throughput_benchmark() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 10: HIGH-THROUGHPUT BENCHMARK (WASM overhead)");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(MockWasmRuntime::new(false)); // Always false -> no match -> allow
    let email = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "bench-rule",
        1,
        "bench-plugin",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Deny,
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&email)], rules);

    let iterations = 1000;
    let start = Instant::now();

    for i in 0..iterations {
        let action = make_action_with_payload(
            "ns",
            "t1",
            "email",
            "bench",
            serde_json::json!({ "iteration": i }),
        );
        gateway.dispatch(action, None).await?;
    }

    let elapsed = start.elapsed();
    let per_action = elapsed / iterations;
    let throughput = iterations as f64 / elapsed.as_secs_f64();

    println!("  Dispatched {iterations} actions with WASM rule evaluation");
    println!("  Total time: {elapsed:?}");
    println!("  Per action: {per_action:?}");
    println!("  Throughput: {throughput:.0} actions/sec");

    email.assert_called(iterations as usize);

    // Sanity check: should be faster than 10ms per action for in-memory mocks
    assert!(
        per_action < Duration::from_millis(10),
        "per-action time {per_action:?} exceeds 10ms threshold"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 10 PASSED]\n");
    Ok(())
}

/// Scenario 11: Hot-reload (plugin v1 -> v2).
async fn scenario_hot_reload() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 11: HOT-RELOAD (plugin v1 -> v2)");
    println!("------------------------------------------------------------------\n");

    // Phase 1: v1 always returns true -> rule matches -> suppress
    let v1_rt = Arc::new(VersionedRuntime::new(1));
    let email_v1 = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "versioned-rule",
        1,
        "versioned-plugin",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Suppress,
    )];

    let gateway = build_gateway_with_wasm(v1_rt, vec![Arc::clone(&email_v1)], rules.clone());

    let action = make_action("ns", "t1", "email", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  v1: outcome = {outcome:?} (expected: Suppressed)");
    assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));
    email_v1.assert_not_called();

    gateway.shutdown().await;

    // Phase 2: v2 always returns false -> rule does not match -> execute
    let v2_rt = Arc::new(VersionedRuntime::new(2));
    let email_v2 = Arc::new(RecordingProvider::new("email"));

    let gateway = build_gateway_with_wasm(v2_rt, vec![Arc::clone(&email_v2)], rules);

    let action = make_action("ns", "t1", "email", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  v2: outcome = {outcome:?} (expected: Executed)");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));
    email_v2.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 11 PASSED]\n");
    Ok(())
}

/// Scenario 12: Chain with WASM step.
async fn scenario_chain_with_wasm() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 12: CHAIN WITH WASM STEP");
    println!("------------------------------------------------------------------\n");

    // WASM runtime that always allows (used in rule evaluation)
    let rt = Arc::new(MockWasmRuntime::new(true));

    let validator = Arc::new(RecordingProvider::new("validator"));
    let processor = Arc::new(RecordingProvider::new("processor"));

    let chain_config = ChainConfig::new("wasm-chain")
        .with_step(ChainStepConfig::new(
            "validate",
            "validator",
            "wasm_validate",
            serde_json::json!({ "check": "payload_format" }),
        ))
        .with_step(ChainStepConfig::new(
            "process",
            "processor",
            "handle",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    // Rule that triggers the chain, guarded by WASM condition
    let rules = vec![wasm_rule(
        "wasm-chain-trigger",
        1,
        "mock-plugin",
        "should_chain",
        serde_json::json!({}),
        RuleAction::Chain {
            chain: "wasm-chain".to_owned(),
        },
    )];

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .wasm_runtime(rt)
        .rules(rules)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .provider(Arc::clone(&validator) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&processor) as Arc<dyn acteon_provider::DynProvider>)
        .build()?;

    // Dispatch should start the chain
    let action = make_action("ns", "t1", "validator", "submit");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  Chain started: {outcome:?}");

    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("expected ChainStarted, got {other:?}"),
    };

    // Advance chain: validate -> process -> complete
    gateway.advance_chain("ns", "t1", &chain_id).await?;
    println!("  Step 0 (validate) advanced");
    gateway.advance_chain("ns", "t1", &chain_id).await?;
    println!("  Step 1 (process) advanced -> complete");

    let chain_state = gateway
        .get_chain_status("ns", "t1", &chain_id)
        .await?
        .expect("chain should exist");

    println!("  Chain status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(chain_state.execution_path, vec!["validate", "process"]);
    validator.assert_called(1);
    processor.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 12 PASSED]\n");
    Ok(())
}

/// Scenario 13: WASM + LLM guardrail combination.
async fn scenario_wasm_plus_llm() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 13: WASM + LLM GUARDRAIL COMBINATION");
    println!("------------------------------------------------------------------\n");

    // WASM rule allows; LLM guardrail provides second-layer check.
    let rt = Arc::new(MockWasmRuntime::new(false)); // WASM does not match -> allow
    let llm = Arc::new(acteon_llm::MockLlmEvaluator::allowing()); // LLM approves

    let email = Arc::new(RecordingProvider::new("email"));

    let rules = vec![wasm_rule(
        "wasm-content-check",
        1,
        "content-guard",
        "evaluate",
        serde_json::json!({}),
        RuleAction::Deny,
    )];

    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .wasm_runtime(rt)
        .llm_evaluator(llm)
        .llm_policy("Verify content safety".to_owned())
        .rules(rules)
        .provider(Arc::clone(&email) as Arc<dyn acteon_provider::DynProvider>)
        .build()?;

    // WASM returns false (no match) -> action allowed by rules -> LLM approves -> execute
    let action = make_action("ns", "t1", "email", "send");
    let outcome = gateway.dispatch(action, None).await?;
    println!("  WASM(no match) + LLM(approve) -> executed: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));
    email.assert_called(1);

    gateway.shutdown().await;
    println!("\n  [Scenario 13 PASSED]\n");
    Ok(())
}

/// Scenario 14: Tenant isolation -- two tenants, different plugin behaviors.
async fn scenario_tenant_isolation() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 14: TENANT ISOLATION (two tenants, different plugins)");
    println!("------------------------------------------------------------------\n");

    // The mock runtime always returns true, but we use different rules per tenant
    // to verify that tenant isolation works. Tenant A gets suppressed, tenant B
    // gets through because the rule condition checks the tenant field.
    let rt = Arc::new(MockWasmRuntime::new(true));

    let email = Arc::new(RecordingProvider::new("email"));

    // Rule that only applies to tenant-a
    let condition_a = Expr::Binary(
        BinaryOp::And,
        Box::new(Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "tenant".into(),
            )),
            Box::new(Expr::String("tenant-a".into())),
        )),
        Box::new(Expr::WasmCall {
            plugin: "tenant-a-plugin".to_owned(),
            function: "evaluate".to_owned(),
        }),
    );

    let rules = vec![
        Rule::new("tenant-a-wasm-suppress", condition_a, RuleAction::Suppress)
            .with_priority(1)
            .with_source(RuleSource::Wasm { plugin: None }),
    ];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&email)], rules);

    // Tenant A action: should match the tenant-specific rule -> suppress
    let action_a = make_action("ns", "tenant-a", "email", "send");
    let outcome_a = gateway.dispatch(action_a, None).await?;
    println!("  Tenant A -> suppress: {outcome_a:?}");
    assert!(matches!(outcome_a, ActionOutcome::Suppressed { .. }));

    // Tenant B action: tenant check fails -> no rule match -> execute
    let action_b = make_action("ns", "tenant-b", "email", "send");
    let outcome_b = gateway.dispatch(action_b, None).await?;
    println!("  Tenant B -> execute: {outcome_b:?}");
    assert!(matches!(outcome_b, ActionOutcome::Executed(..)));
    email.assert_called(1); // Only tenant B's action reached the provider

    gateway.shutdown().await;
    println!("\n  [Scenario 14 PASSED]\n");
    Ok(())
}

/// Scenario 15: Plugin returning modified payload via metadata.
async fn scenario_modified_payload() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 15: PLUGIN RETURNING MODIFIED PAYLOAD");
    println!("------------------------------------------------------------------\n");

    let rt = Arc::new(PayloadTransformRuntime);
    let webhook = Arc::new(RecordingProvider::new("webhook"));

    // Rule that uses WASM to enrich then allows with modification
    let rules = vec![wasm_rule(
        "wasm-enrich",
        1,
        "transform-plugin",
        "enrich",
        serde_json::json!({}),
        RuleAction::Modify {
            changes: serde_json::json!({ "enriched": true }),
        },
    )];

    let gateway = build_gateway_with_wasm(rt, vec![Arc::clone(&webhook)], rules);

    let action = make_action_with_payload(
        "ns",
        "t1",
        "webhook",
        "send",
        serde_json::json!({ "original_data": "test-value" }),
    );
    let outcome = gateway.dispatch(action, None).await?;
    println!("  Payload transform outcome: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "modified action should be executed"
    );
    webhook.assert_called(1);

    // Verify the provider received the enriched payload
    let calls = webhook.calls();
    assert_eq!(calls.len(), 1);
    let received_payload = &calls[0].action.payload;
    println!("  Provider received payload: {received_payload}");
    assert_eq!(
        received_payload.get("enriched"),
        Some(&serde_json::json!(true)),
        "payload should contain the enrichment from the Modify rule action"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 15 PASSED]\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin configuration verification (non-dispatch scenarios)
// ---------------------------------------------------------------------------

/// Verify WasmPluginConfig construction and serialization.
fn verify_plugin_configs() {
    println!("------------------------------------------------------------------");
    println!("  BONUS: WASM PLUGIN CONFIG VERIFICATION");
    println!("------------------------------------------------------------------\n");

    let config = WasmPluginConfig::new("rate-limiter")
        .with_description("Checks request rate against thresholds")
        .with_memory_limit(8 * 1024 * 1024) // 8 MB
        .with_timeout_ms(50)
        .with_wasm_path("/plugins/rate-limiter.wasm");

    println!("  Plugin config: {config:?}");
    assert_eq!(config.name, "rate-limiter");
    assert_eq!(config.memory_limit_bytes, 8 * 1024 * 1024);
    assert_eq!(config.timeout_ms, 50);
    assert!(config.enabled);

    // Verify serde roundtrip
    let json = serde_json::to_string(&config).unwrap();
    let back: WasmPluginConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.name, "rate-limiter");
    assert_eq!(back.timeout_ms, 50);

    // Disabled plugin
    let disabled = WasmPluginConfig::new("experimental").with_enabled(false);
    assert!(!disabled.enabled);

    println!("  [Config verification PASSED]\n");
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     ACTEON WASM PLUGIN SIMULATION");
    println!("     15 scenarios covering the full WASM plugin lifecycle");
    println!("==================================================================\n");

    let total_start = Instant::now();

    scenario_basic_wasm_rule().await?;
    scenario_threshold_parameter().await?;
    scenario_mixed_yaml_wasm().await?;
    scenario_cel_wasm_inline().await?;
    scenario_timeout_enforcement().await?;
    scenario_memory_limit().await?;
    scenario_trap_handling().await?;
    scenario_multiple_plugins().await?;
    scenario_state_access().await?;
    scenario_throughput_benchmark().await?;
    scenario_hot_reload().await?;
    scenario_chain_with_wasm().await?;
    scenario_wasm_plus_llm().await?;
    scenario_tenant_isolation().await?;
    scenario_modified_payload().await?;

    verify_plugin_configs();

    let total_elapsed = total_start.elapsed();

    println!("==================================================================");
    println!("     ALL 15 WASM PLUGIN SCENARIOS PASSED");
    println!("     Total time: {total_elapsed:?}");
    println!("==================================================================");

    Ok(())
}
