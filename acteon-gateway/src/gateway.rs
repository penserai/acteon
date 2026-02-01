use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, instrument};

use acteon_core::{Action, ActionOutcome};
use acteon_executor::ActionExecutor;
use acteon_provider::ProviderRegistry;
use acteon_rules::{EvalContext, RuleEngine, RuleVerdict};
use acteon_state::{DistributedLock, KeyKind, StateKey, StateStore};

use crate::error::GatewayError;
use crate::metrics::GatewayMetrics;

/// The central gateway that orchestrates the action dispatch pipeline.
///
/// The dispatch pipeline for each action:
/// 1. Acquire a distributed lock scoped to the action.
/// 2. Evaluate all rules to produce a [`RuleVerdict`].
/// 3. Execute the verdict (allow, deduplicate, suppress, reroute, throttle, etc.).
/// 4. Release the lock and return the [`ActionOutcome`].
pub struct Gateway {
    // Note: manual `Debug` impl below because trait objects lack `Debug`.
    pub(crate) state: Arc<dyn StateStore>,
    pub(crate) lock: Arc<dyn DistributedLock>,
    pub(crate) engine: RuleEngine,
    pub(crate) providers: ProviderRegistry,
    pub(crate) executor: ActionExecutor,
    pub(crate) environment: HashMap<String, String>,
    pub(crate) metrics: Arc<GatewayMetrics>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("environment", &self.environment)
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// Dispatch a single action through the full gateway pipeline.
    ///
    /// This acquires a per-action distributed lock, evaluates rules, and
    /// executes (or skips) the action according to the resulting verdict.
    #[instrument(
        skip(self),
        fields(
            action.id = %action.id,
            action.namespace = %action.namespace,
            action.provider = %action.provider,
        )
    )]
    pub async fn dispatch(&self, action: Action) -> Result<ActionOutcome, GatewayError> {
        self.metrics.increment_dispatched();

        // 1. Build a lock name scoped to this specific action.
        let lock_name = format!(
            "dispatch:{}:{}:{}",
            action.namespace, action.tenant, action.id
        );

        // 2. Acquire the distributed lock with a 30-second TTL and 5-second timeout.
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        info!("distributed lock acquired");

        // 3. Build the evaluation context and evaluate rules.
        let eval_ctx = EvalContext::new(&action, self.state.as_ref(), &self.environment);
        let verdict = self.engine.evaluate(&eval_ctx).await?;

        info!(?verdict, "rule evaluation complete");

        // 4. Handle the verdict.
        let outcome = match verdict {
            RuleVerdict::Allow => self.execute_action(&action).await,
            RuleVerdict::Deduplicate { ttl_seconds } => {
                self.handle_dedup(&action, ttl_seconds).await?
            }
            RuleVerdict::Suppress(rule) | RuleVerdict::Deny(rule) => {
                self.metrics.increment_suppressed();
                ActionOutcome::Suppressed { rule }
            }
            RuleVerdict::Reroute {
                rule: _,
                target_provider,
            } => self.handle_reroute(&action, &target_provider).await?,
            RuleVerdict::Throttle {
                rule: _,
                max_count: _,
                window_seconds,
            } => {
                self.metrics.increment_throttled();
                ActionOutcome::Throttled {
                    retry_after: Duration::from_secs(window_seconds),
                }
            }
            RuleVerdict::Modify { rule: _, changes } => {
                let mut modified = action.clone();
                json_patch::merge(&mut modified.payload, &changes);
                self.execute_action(&modified).await
            }
        };

        // 5. Release the lock explicitly.
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        info!(?outcome, "dispatch complete");

        Ok(outcome)
    }

    /// Dispatch a batch of actions sequentially, collecting results.
    pub async fn dispatch_batch(
        &self,
        actions: Vec<Action>,
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        let mut results = Vec::with_capacity(actions.len());
        for action in actions {
            results.push(self.dispatch(action).await);
        }
        results
    }

    /// Return a reference to the gateway metrics.
    pub fn metrics(&self) -> &GatewayMetrics {
        &self.metrics
    }

    /// Replace the rule engine's rules with a new set, re-sorting by priority.
    pub fn reload_rules(&mut self, rules: Vec<acteon_rules::Rule>) {
        self.engine = RuleEngine::new(rules);
    }

    /// Return a reference to the sorted rules in the engine.
    pub fn rules(&self) -> &[acteon_rules::Rule] {
        self.engine.rules()
    }

    /// Enable a rule by name. Returns `true` if the rule was found.
    pub fn enable_rule(&mut self, name: &str) -> bool {
        self.engine.enable_rule(name)
    }

    /// Disable a rule by name. Returns `true` if the rule was found.
    pub fn disable_rule(&mut self, name: &str) -> bool {
        self.engine.disable_rule(name)
    }

    /// Load rules from a directory using the given frontends, replacing current rules.
    pub fn load_rules_from_directory(
        &mut self,
        path: &std::path::Path,
        frontends: &[&dyn acteon_rules::RuleFrontend],
    ) -> Result<usize, GatewayError> {
        self.engine = RuleEngine::new(vec![]);
        self.engine
            .load_directory(path, frontends)
            .map_err(|e| GatewayError::Configuration(e.to_string()))
    }

    // -- Private helpers ------------------------------------------------------

    /// Look up the action's provider and execute through the executor.
    async fn execute_action(&self, action: &Action) -> ActionOutcome {
        let Some(provider) = self.providers.get(action.provider.as_str()) else {
            self.metrics.increment_failed();
            return ActionOutcome::Failed(acteon_core::ActionError {
                code: "PROVIDER_NOT_FOUND".into(),
                message: format!("provider not found: {}", action.provider),
                retryable: false,
                attempts: 0,
            });
        };
        let result = self.executor.execute(action, provider.as_ref()).await;
        match &result {
            ActionOutcome::Executed(_) => self.metrics.increment_executed(),
            ActionOutcome::Failed(_) => self.metrics.increment_failed(),
            _ => {}
        }
        result
    }

    /// Handle the deduplication verdict: check state, execute only if new.
    async fn handle_dedup(
        &self,
        action: &Action,
        ttl_seconds: Option<u64>,
    ) -> Result<ActionOutcome, GatewayError> {
        let dedup_key = action
            .dedup_key
            .as_deref()
            .unwrap_or_else(|| action.id.as_str());

        let state_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::Dedup,
            dedup_key,
        );

        let ttl = ttl_seconds.map(Duration::from_secs);
        let is_new = self.state.check_and_set(&state_key, "1", ttl).await?;

        if is_new {
            Ok(self.execute_action(action).await)
        } else {
            self.metrics.increment_deduplicated();
            Ok(ActionOutcome::Deduplicated)
        }
    }

    /// Handle the reroute verdict: execute with the target provider.
    async fn handle_reroute(
        &self,
        action: &Action,
        target_provider: &str,
    ) -> Result<ActionOutcome, GatewayError> {
        let provider = self
            .providers
            .get(target_provider)
            .ok_or_else(|| GatewayError::ProviderNotFound(target_provider.to_owned()))?;

        let result = self.executor.execute(action, provider.as_ref()).await;
        match &result {
            ActionOutcome::Executed(resp) => {
                self.metrics.increment_rerouted();
                Ok(ActionOutcome::Rerouted {
                    original_provider: action.provider.to_string(),
                    new_provider: target_provider.to_owned(),
                    response: resp.clone(),
                })
            }
            ActionOutcome::Failed(_) => {
                self.metrics.increment_failed();
                Ok(result)
            }
            _ => Ok(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;

    use acteon_core::{Action, ActionOutcome, ProviderResponse};
    use acteon_executor::ExecutorConfig;
    use acteon_provider::{DynProvider, ProviderError};
    use acteon_rules::ir::expr::Expr;
    use acteon_rules::ir::rule::{Rule, RuleAction};
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

    use crate::builder::GatewayBuilder;

    // -- Mock provider --------------------------------------------------------

    struct MockProvider {
        provider_name: String,
    }

    impl MockProvider {
        fn new(name: &str) -> Self {
            Self {
                provider_name: name.to_owned(),
            }
        }
    }

    #[async_trait]
    impl DynProvider for MockProvider {
        fn name(&self) -> &str {
            &self.provider_name
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    // -- Helpers --------------------------------------------------------------

    fn test_action() -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({"to": "user@example.com"}),
        )
    }

    fn build_gateway(rules: Vec<Rule>) -> crate::gateway::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(rules)
            .provider(Arc::new(MockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .build()
            .expect("gateway should build")
    }

    // -- Capturing provider ---------------------------------------------------

    struct CapturingProvider {
        provider_name: String,
        captured: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    impl CapturingProvider {
        fn new(name: &str) -> (Self, Arc<Mutex<Vec<serde_json::Value>>>) {
            let captured = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    provider_name: name.to_owned(),
                    captured: Arc::clone(&captured),
                },
                captured,
            )
        }
    }

    #[async_trait]
    impl DynProvider for CapturingProvider {
        fn name(&self) -> &str {
            &self.provider_name
        }

        async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
            self.captured.lock().unwrap().push(action.payload.clone());
            Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn build_capturing_gateway(
        rules: Vec<Rule>,
    ) -> (crate::gateway::Gateway, Arc<Mutex<Vec<serde_json::Value>>>) {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let (provider, captured) = CapturingProvider::new("email");

        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(rules)
            .provider(Arc::new(provider))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .build()
            .expect("gateway should build");
        (gw, captured)
    }

    // -- Tests ----------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_allow_no_rules() {
        let gw = build_gateway(vec![]);
        let outcome = gw.dispatch(test_action()).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "no rules should default to Allow and execute"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.dispatched, 1);
        assert_eq!(snap.executed, 1);
    }

    #[tokio::test]
    async fn dispatch_dedup_second_is_deduplicated() {
        let rules = vec![Rule::new(
            "dedup",
            Expr::Bool(true),
            RuleAction::Deduplicate {
                ttl_seconds: Some(300),
            },
        )];
        let gw = build_gateway(rules);

        let mut action = test_action();
        action.dedup_key = Some("unique-key".into());

        // First dispatch should execute.
        let outcome1 = gw.dispatch(action.clone()).await.unwrap();
        assert!(matches!(outcome1, ActionOutcome::Executed(_)));

        // Second dispatch with same dedup key should be deduplicated.
        let outcome2 = gw.dispatch(action).await.unwrap();
        assert!(matches!(outcome2, ActionOutcome::Deduplicated));

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.dispatched, 2);
        assert_eq!(snap.executed, 1);
        assert_eq!(snap.deduplicated, 1);
    }

    #[tokio::test]
    async fn dispatch_suppress() {
        let rules = vec![Rule::new(
            "block-all",
            Expr::Bool(true),
            RuleAction::Suppress,
        )];
        let gw = build_gateway(rules);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.suppressed, 1);
    }

    #[tokio::test]
    async fn dispatch_deny_maps_to_suppressed() {
        let rules = vec![Rule::new("deny-all", Expr::Bool(true), RuleAction::Deny)];
        let gw = build_gateway(rules);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        match outcome {
            ActionOutcome::Suppressed { rule } => {
                assert_eq!(rule, "deny-all");
            }
            other => panic!("expected Suppressed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_reroute() {
        let rules = vec![Rule::new(
            "reroute-sms",
            Expr::Bool(true),
            RuleAction::Reroute {
                target_provider: "sms-fallback".into(),
            },
        )];
        let gw = build_gateway(rules);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        match outcome {
            ActionOutcome::Rerouted {
                original_provider,
                new_provider,
                ..
            } => {
                assert_eq!(original_provider, "email");
                assert_eq!(new_provider, "sms-fallback");
            }
            other => panic!("expected Rerouted, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.rerouted, 1);
    }

    #[tokio::test]
    async fn dispatch_throttle() {
        let rules = vec![Rule::new(
            "rate-limit",
            Expr::Bool(true),
            RuleAction::Throttle {
                max_count: 100,
                window_seconds: 60,
            },
        )];
        let gw = build_gateway(rules);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        match outcome {
            ActionOutcome::Throttled { retry_after } => {
                assert_eq!(retry_after, Duration::from_secs(60));
            }
            other => panic!("expected Throttled, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.throttled, 1);
    }

    #[tokio::test]
    async fn dispatch_provider_not_found() {
        let gw = build_gateway(vec![]);

        // Action targeting a provider that is not registered.
        let mut action = test_action();
        action.provider = "nonexistent".into();

        let outcome = gw.dispatch(action).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Failed(_)),
            "missing provider should produce Failed outcome"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.failed, 1);
    }

    #[tokio::test]
    async fn dispatch_reroute_provider_not_found() {
        let rules = vec![Rule::new(
            "reroute-missing",
            Expr::Bool(true),
            RuleAction::Reroute {
                target_provider: "does-not-exist".into(),
            },
        )];
        let gw = build_gateway(rules);

        let result = gw.dispatch(test_action()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("does-not-exist"),
            "error should mention the missing provider"
        );
    }

    #[tokio::test]
    async fn dispatch_modify_stub_executes() {
        let rules = vec![Rule::new(
            "modify-stub",
            Expr::Bool(true),
            RuleAction::Modify {
                changes: serde_json::json!({"priority": "high"}),
            },
        )];
        let (gw, captured) = build_capturing_gateway(rules);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "modify should execute the action"
        );

        let payloads = captured.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["priority"], "high");
        assert_eq!(payloads[0]["to"], "user@example.com");
    }

    #[tokio::test]
    async fn dispatch_modify_changes_payload() {
        let rules = vec![Rule::new(
            "modify-payload",
            Expr::Bool(true),
            RuleAction::Modify {
                changes: serde_json::json!({"priority": "high", "to": "admin@example.com"}),
            },
        )];
        let (gw, captured) = build_capturing_gateway(rules);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "modify should execute the action"
        );

        let payloads = captured.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        // The original payload had {"to": "user@example.com"}.
        // The merge patch overwrites "to" and adds "priority".
        assert_eq!(payloads[0]["to"], "admin@example.com");
        assert_eq!(payloads[0]["priority"], "high");
    }

    #[tokio::test]
    async fn dispatch_batch_collects_results() {
        let gw = build_gateway(vec![]);

        let actions = vec![test_action(), test_action(), test_action()];
        let results = gw.dispatch_batch(actions).await;

        assert_eq!(results.len(), 3);
        for result in &results {
            assert!(result.is_ok());
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.dispatched, 3);
        assert_eq!(snap.executed, 3);
    }

    #[tokio::test]
    async fn reload_rules_takes_effect() {
        let mut gw = build_gateway(vec![]);

        // Initially no rules -- action is executed.
        let outcome = gw.dispatch(test_action()).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Executed(_)));

        // Reload with a suppress rule.
        gw.reload_rules(vec![Rule::new(
            "block",
            Expr::Bool(true),
            RuleAction::Suppress,
        )]);

        let outcome = gw.dispatch(test_action()).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));
    }

    #[tokio::test]
    async fn metrics_increment_correctly() {
        let gw = build_gateway(vec![]);

        // Dispatch several actions.
        for _ in 0..5 {
            let _ = gw.dispatch(test_action()).await;
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.dispatched, 5);
        assert_eq!(snap.executed, 5);
        assert_eq!(snap.deduplicated, 0);
        assert_eq!(snap.suppressed, 0);
        assert_eq!(snap.rerouted, 0);
        assert_eq!(snap.throttled, 0);
        assert_eq!(snap.failed, 0);
    }
}
