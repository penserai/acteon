use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, instrument, warn};

use acteon_audit::AuditRecord;
use acteon_audit::store::AuditStore;
use acteon_core::{
    Action, ActionOutcome, Caller, ChainConfig, ChainState, ChainStatus, ChainStepConfig,
    StateMachineConfig, StepResult, StreamEvent, StreamEventType, compute_fingerprint,
    sanitize_outcome,
};
use acteon_executor::{ActionExecutor, DeadLetterEntry, DeadLetterSink};
use acteon_provider::ProviderRegistry;
use acteon_rules::{EvalContext, RuleEngine, RuleVerdict};
use acteon_state::{DistributedLock, KeyKind, StateKey, StateStore};

use serde::{Deserialize, Serialize};

use crate::circuit_breaker::CircuitBreakerRegistry;
use crate::group_manager::GroupManager;

use crate::error::GatewayError;
use crate::metrics::GatewayMetrics;

type HmacSha256 = Hmac<Sha256>;

/// A named HMAC key for signing/verifying approval URLs.
#[derive(Debug, Clone)]
pub struct ApprovalKey {
    /// Key identifier included in signed URLs.
    pub kid: String,
    /// The raw HMAC secret bytes.
    pub secret: Vec<u8>,
}

/// Ordered set of HMAC keys. Index 0 = current signing key.
#[derive(Debug, Clone)]
pub struct ApprovalKeySet {
    keys: Vec<ApprovalKey>,
}

impl ApprovalKeySet {
    /// Create a new key set. Panics if `keys` is empty.
    pub fn new(keys: Vec<ApprovalKey>) -> Self {
        assert!(
            !keys.is_empty(),
            "ApprovalKeySet must have at least one key"
        );
        Self { keys }
    }

    /// Create a key set from a single secret (legacy compatibility). Uses kid `"k0"`.
    pub fn from_single(secret: Vec<u8>) -> Self {
        Self {
            keys: vec![ApprovalKey {
                kid: "k0".into(),
                secret,
            }],
        }
    }

    /// The current signing key (first in the list).
    pub fn current(&self) -> &ApprovalKey {
        &self.keys[0]
    }

    /// Look up a key by its kid.
    pub fn get(&self, kid: &str) -> Option<&ApprovalKey> {
        self.keys.iter().find(|k| k.kid == kid)
    }

    /// All keys (for try-all verification).
    pub fn all(&self) -> &[ApprovalKey] {
        &self.keys
    }
}

/// A stored approval record awaiting human decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// The original action to execute on approval.
    pub action: Action,
    /// The approval ID (UUID).
    pub token: String,
    /// Name of the rule that triggered the approval request.
    pub rule: String,
    /// When the approval request was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the approval request expires.
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// Current status: "pending", "approved", or "rejected".
    pub status: String,
    /// Who approved/rejected (if decided).
    pub decided_by: Option<String>,
    /// When the decision was made.
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional message from the rule.
    pub message: Option<String>,
    /// Whether the notification was successfully sent.
    #[serde(default)]
    pub notification_sent: bool,
}

/// Public-facing approval status (does not expose the original action payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalStatus {
    /// The approval token.
    pub token: String,
    /// Current status.
    pub status: String,
    /// Rule that triggered the approval.
    pub rule: String,
    /// When the approval was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the approval expires.
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// When the decision was made.
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional message.
    pub message: Option<String>,
}

/// Internal wrapper for quota policies cached in memory.
#[derive(Debug, Clone)]
pub(crate) struct CachedPolicy {
    pub(crate) policy: acteon_core::QuotaPolicy,
    pub(crate) cached_at: chrono::DateTime<Utc>,
}

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
    pub(crate) audit: Option<Arc<dyn AuditStore>>,
    pub(crate) audit_ttl_seconds: Option<u64>,
    pub(crate) audit_store_payload: bool,
    pub(crate) audit_tracker: TaskTracker,
    pub(crate) dlq: Option<Arc<dyn DeadLetterSink>>,
    pub(crate) state_machines: HashMap<String, StateMachineConfig>,
    pub(crate) group_manager: Arc<GroupManager>,
    pub(crate) external_url: Option<String>,
    pub(crate) approval_keys: ApprovalKeySet,
    pub(crate) llm_evaluator: Option<Arc<dyn acteon_llm::LlmEvaluator>>,
    pub(crate) llm_policy: String,
    pub(crate) llm_policies: HashMap<String, String>,
    pub(crate) llm_fail_open: bool,
    pub(crate) chains: HashMap<String, ChainConfig>,
    /// Pre-computed step-name-to-index maps for each chain config, built once at
    /// gateway construction time to avoid repeated `HashMap` allocations during
    /// chain advancement.
    pub(crate) chain_step_indices: HashMap<String, HashMap<String, usize>>,
    pub(crate) completed_chain_ttl: Option<Duration>,
    pub(crate) embedding: Option<Arc<dyn acteon_rules::EmbeddingEvalSupport>>,
    pub(crate) default_timezone: Option<chrono_tz::Tz>,
    pub(crate) circuit_breakers: Option<crate::circuit_breaker::CircuitBreakerRegistry>,
    /// Broadcast channel for real-time SSE event streaming.
    pub(crate) stream_tx: tokio::sync::broadcast::Sender<StreamEvent>,
    /// Quota policies indexed by `"namespace:tenant"`.
    ///
    /// Wrapped in a `RwLock` so that [`check_quota`](Self::check_quota) can
    /// lazily cache policies discovered from the state store (hot-reload
    /// visibility across distributed instances).
    pub(crate) quota_policies: parking_lot::RwLock<HashMap<String, CachedPolicy>>,
    /// Optional payload encryptor for encrypting action payloads at rest.
    pub(crate) payload_encryptor: Option<Arc<acteon_crypto::PayloadEncryptor>>,
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
    /// Returns a reference to the payload encryptor, if configured.
    pub fn payload_encryptor(&self) -> Option<&acteon_crypto::PayloadEncryptor> {
        self.payload_encryptor.as_deref()
    }

    /// Encrypt a state value if a payload encryptor is configured, otherwise passthrough.
    pub fn encrypt_state_value(&self, value: &str) -> Result<String, GatewayError> {
        match self.payload_encryptor {
            Some(ref enc) => enc.encrypt_str(value).map_err(|e| {
                GatewayError::Configuration(format!("payload encryption failed: {e}"))
            }),
            None => Ok(value.to_owned()),
        }
    }

    /// Decrypt a state value if a payload encryptor is configured, otherwise passthrough.
    pub fn decrypt_state_value(&self, value: &str) -> Result<String, GatewayError> {
        match self.payload_encryptor {
            Some(ref enc) => enc.decrypt_str(value).map_err(|e| {
                GatewayError::Configuration(format!("payload decryption failed: {e}"))
            }),
            None => Ok(value.to_owned()),
        }
    }

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
    pub async fn dispatch(
        &self,
        action: Action,
        caller: Option<&Caller>,
    ) -> Result<ActionOutcome, GatewayError> {
        self.dispatch_inner(action, caller, false).await
    }

    /// Dispatch in dry-run mode: evaluates rules and returns the verdict without
    /// executing, recording state, or emitting audit records.
    pub async fn dispatch_dry_run(
        &self,
        action: Action,
        caller: Option<&Caller>,
    ) -> Result<ActionOutcome, GatewayError> {
        self.dispatch_inner(action, caller, true).await
    }

    /// Inner dispatch implementation shared by normal and dry-run modes.
    #[allow(clippy::too_many_lines)]
    #[instrument(
        name = "gateway.dispatch",
        skip(self, action, caller),
        fields(
            action.id = %action.id,
            action.namespace = %action.namespace,
            action.tenant = %action.tenant,
            action.provider = %action.provider,
            action.action_type = %action.action_type,
            dry_run,
        )
    )]
    async fn dispatch_inner(
        &self,
        action: Action,
        caller: Option<&Caller>,
        dry_run: bool,
    ) -> Result<ActionOutcome, GatewayError> {
        self.metrics.increment_dispatched();
        let start = std::time::Instant::now();
        let dispatched_at = Utc::now();
        let event_id = uuid::Uuid::now_v7().to_string();

        // 1. Build a lock name scoped to this specific action.
        let lock_name = format!(
            "dispatch:{}:{}:{}",
            action.namespace, action.tenant, action.id
        );

        // In dry-run mode, skip lock acquisition, state mutation, and audit.
        let guard = if dry_run {
            None
        } else {
            // 2. Acquire the distributed lock with a 30-second TTL and 5-second timeout.
            Some(
                self.lock
                    .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
                    .await
                    .map_err(|e| GatewayError::LockFailed(e.to_string()))?,
            )
        };

        if !dry_run {
            info!("distributed lock acquired");
        }

        // 2b. Quota check (skip in dry-run mode).
        if !dry_run && let Some(outcome) = self.check_quota(&action).await? {
            let dummy_verdict = RuleVerdict::Allow(None);
            if let Some(ref audit) = self.audit {
                let record = build_audit_record(
                    event_id.clone(),
                    &action,
                    &dummy_verdict,
                    &outcome,
                    dispatched_at,
                    start.elapsed(),
                    self.audit_ttl_seconds,
                    self.audit_store_payload,
                    caller,
                );
                let audit = Arc::clone(audit);
                self.audit_tracker.spawn(async move {
                    if let Err(e) = audit.record(record).await {
                        warn!(error = %e, "audit recording failed");
                    }
                });
            }
            let stream_event = StreamEvent {
                id: event_id,
                timestamp: dispatched_at,
                event_type: StreamEventType::ActionDispatched {
                    outcome: sanitize_outcome(&outcome),
                    provider: action.provider.to_string(),
                },
                namespace: action.namespace.to_string(),
                tenant: action.tenant.to_string(),
                action_type: Some(action.action_type.clone()),
                action_id: Some(action.id.to_string()),
            };
            let _ = self.stream_tx.send(stream_event);
            if let Some(g) = guard {
                let _ = g.release().await;
            }
            return Ok(outcome);
        }

        // 3. Build the evaluation context and evaluate rules.
        let mut eval_ctx = EvalContext::new(&action, self.state.as_ref(), &self.environment);
        if let Some(ref emb) = self.embedding {
            eval_ctx = eval_ctx.with_embedding(Arc::clone(emb));
        }
        if let Some(tz) = self.default_timezone {
            eval_ctx = eval_ctx.with_timezone(tz);
        }
        let verdict = self.engine.evaluate(&eval_ctx).await?;

        info!(?verdict, "rule evaluation complete");

        // 3b. LLM guardrail check (skipped for already-denied/suppressed verdicts).
        let verdict = self.apply_llm_guardrail(&action, verdict).await;

        // 3c. In dry-run mode, return early with the verdict without executing.
        if dry_run {
            let would_be_provider = match &verdict {
                RuleVerdict::Reroute {
                    target_provider, ..
                } => target_provider.clone(),
                _ => action.provider.to_string(),
            };
            return Ok(ActionOutcome::DryRun {
                verdict: verdict.as_tag().to_owned(),
                matched_rule: matched_rule_name(&verdict),
                would_be_provider,
            });
        }

        // 4. Handle the verdict.
        let outcome = match &verdict {
            RuleVerdict::Allow(_) => self.execute_action(&action).await,
            RuleVerdict::Deduplicate { ttl_seconds } => {
                self.handle_dedup(&action, *ttl_seconds).await?
            }
            RuleVerdict::Suppress(rule) | RuleVerdict::Deny(rule) => {
                self.metrics.increment_suppressed();
                ActionOutcome::Suppressed { rule: rule.clone() }
            }
            RuleVerdict::Reroute {
                rule: _,
                target_provider,
            } => self.handle_reroute(&action, target_provider).await?,
            RuleVerdict::Throttle {
                rule: _,
                max_count: _,
                window_seconds,
            } => {
                self.metrics.increment_throttled();
                ActionOutcome::Throttled {
                    retry_after: Duration::from_secs(*window_seconds),
                }
            }
            RuleVerdict::Modify { rule: _, changes } => {
                let mut modified = action.clone();
                json_patch::merge(&mut modified.payload, changes);
                self.execute_action(&modified).await
            }
            RuleVerdict::StateMachine {
                rule: _,
                state_machine,
                fingerprint_fields,
            } => {
                self.handle_state_machine(&action, state_machine, fingerprint_fields)
                    .await?
            }
            RuleVerdict::Group {
                rule: _,
                group_by,
                group_wait_seconds,
                group_interval_seconds,
                max_group_size,
                template: _,
            } => {
                self.handle_group(
                    &action,
                    group_by,
                    *group_wait_seconds,
                    *group_interval_seconds,
                    *max_group_size,
                )
                .await?
            }
            RuleVerdict::RequestApproval {
                rule,
                notify_provider,
                timeout_seconds,
                message,
            } => {
                self.handle_request_approval(
                    &action,
                    rule,
                    notify_provider,
                    *timeout_seconds,
                    message.as_deref(),
                )
                .await?
            }
            RuleVerdict::Chain { rule: _, chain } => self.handle_chain(&action, chain).await?,
            RuleVerdict::Schedule {
                rule: _,
                delay_seconds,
            } => self.handle_schedule(&action, *delay_seconds).await?,
        };

        // 5. Emit audit record (tracked async task for graceful shutdown).
        if let Some(ref audit) = self.audit {
            let record = build_audit_record(
                event_id.clone(),
                &action,
                &verdict,
                &outcome,
                dispatched_at,
                start.elapsed(),
                self.audit_ttl_seconds,
                self.audit_store_payload,
                caller,
            );
            let audit = Arc::clone(audit);
            self.audit_tracker.spawn(async move {
                if let Err(e) = audit.record(record).await {
                    warn!(error = %e, "audit recording failed");
                }
            });
        }

        // 6. Emit SSE stream event (fire-and-forget; no-op if no subscribers).
        //    The outcome is sanitized to strip provider response bodies,
        //    headers, and HMAC-signed approval URLs before broadcasting.
        if !dry_run {
            let stream_event = StreamEvent {
                id: event_id,
                timestamp: dispatched_at,
                event_type: StreamEventType::ActionDispatched {
                    outcome: sanitize_outcome(&outcome),
                    provider: action.provider.to_string(),
                },
                namespace: action.namespace.to_string(),
                tenant: action.tenant.to_string(),
                action_type: Some(action.action_type.clone()),
                action_id: Some(action.id.to_string()),
            };
            let _ = self.stream_tx.send(stream_event);
        }

        // 7. Release the lock explicitly.
        if let Some(guard) = guard {
            guard
                .release()
                .await
                .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        }

        info!(?outcome, "dispatch complete");

        Ok(outcome)
    }

    /// Dispatch a batch of actions in parallel, collecting results.
    ///
    /// Actions are processed concurrently up to the executor's concurrency limit.
    /// Results are returned in the same order as the input actions.
    pub async fn dispatch_batch(
        &self,
        actions: Vec<Action>,
        caller: Option<&Caller>,
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        self.dispatch_batch_inner(actions, caller, false).await
    }

    /// Dispatch a batch in dry-run mode.
    pub async fn dispatch_batch_dry_run(
        &self,
        actions: Vec<Action>,
        caller: Option<&Caller>,
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        self.dispatch_batch_inner(actions, caller, true).await
    }

    /// Inner batch dispatch implementation.
    async fn dispatch_batch_inner(
        &self,
        actions: Vec<Action>,
        caller: Option<&Caller>,
        dry_run: bool,
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        use futures::stream::{self, StreamExt};

        // Process actions in parallel with bounded concurrency.
        // The executor already has its own concurrency limits, so we use a
        // reasonable batch concurrency here (e.g., 32 concurrent dispatches).
        const BATCH_CONCURRENCY: usize = 32;

        stream::iter(actions)
            .map(|action| self.dispatch_inner(action, caller, dry_run))
            .buffer_unordered(BATCH_CONCURRENCY)
            .collect()
            .await
    }

    /// Resolve the LLM policy string for the given action and verdict.
    ///
    /// Resolution order (most specific wins):
    /// 1. Rule metadata `llm_policy` key (per-rule override)
    /// 2. `self.llm_policies[action.action_type]` (per-action-type override)
    /// 3. `self.llm_policy` (global default)
    fn resolve_llm_policy(&self, action: &Action, verdict: &RuleVerdict) -> String {
        // 1. Check rule metadata for llm_policy.
        if let Some(rule_name) = verdict.rule_name()
            && let Some(rule) = self.engine.rule_by_name(rule_name)
            && let Some(policy) = rule.metadata.get("llm_policy")
        {
            return policy.clone();
        }

        // 2. Check per-action-type policy map.
        if let Some(policy) = self.llm_policies.get(&action.action_type) {
            return policy.clone();
        }

        // 3. Global default.
        self.llm_policy.clone()
    }

    /// Apply the optional LLM guardrail to a verdict.
    ///
    /// Skips the LLM call if no evaluator is configured or if the verdict
    /// is already `Deny` or `Suppress`. On error, behaviour depends on
    /// `llm_fail_open`.
    #[instrument(name = "gateway.llm_guardrail", skip_all)]
    async fn apply_llm_guardrail(&self, action: &Action, verdict: RuleVerdict) -> RuleVerdict {
        let Some(ref llm) = self.llm_evaluator else {
            return verdict;
        };

        // Skip LLM evaluation for already-denied/suppressed actions.
        if matches!(verdict, RuleVerdict::Deny(_) | RuleVerdict::Suppress(_)) {
            return verdict;
        }

        let policy = self.resolve_llm_policy(action, &verdict);

        match llm.evaluate(action, &policy).await {
            Ok(response) => {
                if response.allowed {
                    self.metrics.increment_llm_guardrail_allowed();
                    verdict
                } else {
                    self.metrics.increment_llm_guardrail_denied();
                    info!(reason = %response.reason, "LLM guardrail denied action");
                    RuleVerdict::Deny(format!("LLM guardrail: {}", response.reason))
                }
            }
            Err(e) => {
                self.metrics.increment_llm_guardrail_errors();
                if self.llm_fail_open {
                    warn!(error = %e, "LLM guardrail error (fail-open), allowing action");
                    verdict
                } else {
                    warn!(error = %e, "LLM guardrail error (fail-closed), denying action");
                    RuleVerdict::Deny(format!("LLM guardrail unavailable: {e}"))
                }
            }
        }
    }

    /// Return a reference to the gateway metrics.
    pub fn metrics(&self) -> &GatewayMetrics {
        &self.metrics
    }

    /// Return a reference to the circuit breaker registry, if configured.
    pub fn circuit_breakers(&self) -> Option<&crate::circuit_breaker::CircuitBreakerRegistry> {
        self.circuit_breakers.as_ref()
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

    /// Gracefully shut down the gateway, waiting for all pending audit tasks.
    ///
    /// This method closes the audit task tracker (preventing new tasks from
    /// being spawned) and waits for all in-flight audit recording tasks to
    /// complete. Call this during server shutdown to avoid losing audit data.
    pub async fn shutdown(&self) {
        self.audit_tracker.close();
        self.audit_tracker.wait().await;
        info!("gateway shutdown complete");
    }

    /// Return the number of entries in the dead-letter queue.
    ///
    /// Returns `None` if the DLQ is not enabled.
    pub async fn dlq_len(&self) -> Option<usize> {
        if let Some(ref dlq) = self.dlq {
            Some(dlq.len().await)
        } else {
            None
        }
    }

    /// Return `true` if the dead-letter queue is empty or not enabled.
    pub async fn dlq_is_empty(&self) -> bool {
        if let Some(ref dlq) = self.dlq {
            dlq.is_empty().await
        } else {
            true
        }
    }

    /// Drain all entries from the dead-letter queue.
    ///
    /// Returns an empty vector if the DLQ is not enabled.
    pub async fn dlq_drain(&self) -> Vec<DeadLetterEntry> {
        if let Some(ref dlq) = self.dlq {
            dlq.drain().await
        } else {
            Vec::new()
        }
    }

    /// Return `true` if the dead-letter queue is enabled.
    pub fn dlq_enabled(&self) -> bool {
        self.dlq.is_some()
    }

    /// Return a snapshot of the current in-memory quota policies.
    pub fn quota_policies(&self) -> HashMap<String, acteon_core::QuotaPolicy> {
        self.quota_policies
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.policy.clone()))
            .collect()
    }

    /// Add or replace a quota policy. Keyed by `"namespace:tenant"`.
    pub fn set_quota_policy(&self, policy: acteon_core::QuotaPolicy) {
        let key = format!("{}:{}", policy.namespace, policy.tenant);
        let cached = CachedPolicy {
            policy,
            cached_at: Utc::now(),
        };
        self.quota_policies.write().insert(key, cached);
    }

    /// Remove a quota policy by its lookup key (`"namespace:tenant"`).
    pub fn remove_quota_policy(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Option<acteon_core::QuotaPolicy> {
        let key = format!("{namespace}:{tenant}");
        self.quota_policies.write().remove(&key).map(|c| c.policy)
    }

    /// Rule Playground where users test actions against the current rule set.
    ///
    /// **Note:** The Playground reads live production state (throttle counters,
    /// state-get values, etc.) unless overrides are provided in `mock_state`.
    /// Its results will change as production state changes. It is not a fully
    /// sandboxed environment.
    ///
    /// When `evaluate_all` is `true`, every enabled rule's condition is
    /// evaluated even after a match, giving a complete picture of how the
    /// entire rule set responds.
    ///
    /// When `evaluate_at` is `Some`, the provided timestamp overrides the
    /// evaluation clock, allowing time-travel debugging of time-sensitive
    /// rules (maintenance windows, weekday restrictions, etc.).
    pub async fn evaluate_rules(
        &self,
        action: &acteon_core::Action,
        include_disabled: bool,
        evaluate_all: bool,
        evaluate_at: Option<chrono::DateTime<chrono::Utc>>,
        mock_state: HashMap<String, String>,
    ) -> Result<acteon_rules::RuleEvaluationTrace, GatewayError> {
        let state_store: Box<dyn acteon_state::StateStore> = if mock_state.is_empty() {
            // No overrides: use the real store directly.
            Box::new(BorrowedStateStore(self.state.as_ref()))
        } else {
            Box::new(PlaygroundStateStore {
                inner: self.state.as_ref(),
                overrides: mock_state,
            })
        };

        let mut eval_ctx = EvalContext::new(action, state_store.as_ref(), &self.environment);
        if let Some(ts) = evaluate_at {
            eval_ctx = eval_ctx.with_now(ts);
        }
        if let Some(ref emb) = self.embedding {
            eval_ctx = eval_ctx.with_embedding(std::sync::Arc::clone(emb));
        }
        if let Some(tz) = self.default_timezone {
            eval_ctx = eval_ctx.with_timezone(tz);
        }
        let mut trace = self
            .engine
            .evaluate_with_trace(&eval_ctx, include_disabled, evaluate_all)
            .await?;

        // If the matched rule is a Modify action, compute the resulting payload
        // by applying the JSON merge patch so the user can inspect the diff.
        if trace.verdict == "modify"
            && let Some(ref matched_name) = trace.matched_rule
            && let Some(rule) = self.engine.rules().iter().find(|r| &r.name == matched_name)
            && let acteon_rules::RuleAction::Modify { changes } = &rule.action
        {
            let mut patched = action.payload.clone();
            json_patch::merge(&mut patched, changes);
            trace.modified_payload = Some(patched);
        }

        // In evaluate_all mode, compute per-rule modify patches and a running
        // cumulative payload preview for each matched Modify rule.
        if evaluate_all {
            let mut running_payload = action.payload.clone();
            for entry in &mut trace.trace {
                if entry.result == acteon_rules::RuleTraceResult::Matched
                    && entry.action == "modify"
                    && let Some(rule) = self
                        .engine
                        .rules()
                        .iter()
                        .find(|r| r.name == entry.rule_name)
                    && let acteon_rules::RuleAction::Modify { changes } = &rule.action
                {
                    entry.modify_patch = Some(changes.clone());
                    json_patch::merge(&mut running_payload, changes);
                    entry.modified_payload_preview = Some(running_payload.clone());
                }
            }
        }

        Ok(trace)
    }

    /// Check whether the action's tenant has exceeded their quota.
    ///
    /// Uses an atomic `increment()` to avoid read-then-write races between
    /// concurrent actions for the same tenant.  The counter is always
    /// incremented first; if the new value exceeds the limit the configured
    /// [`OverageBehavior`](acteon_core::OverageBehavior) determines the outcome.
    ///
    /// Returns `None` when the action is within quota or no policy exists.
    /// Returns `Some(ActionOutcome::QuotaExceeded { .. })` when the action
    /// should be blocked or degraded.
    #[instrument(name = "gateway.check_quota", skip_all)]
    async fn check_quota(&self, action: &Action) -> Result<Option<ActionOutcome>, GatewayError> {
        // Skip quota for internal re-dispatches (scheduled, recurring, groups)
        // to avoid double-counting. The action was already counted when it
        // first entered the gateway.
        if action
            .payload
            .get("_scheduled_dispatch")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            || action
                .payload
                .get("_recurring_dispatch")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            || action
                .payload
                .get("_group_dispatch")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            return Ok(None);
        }

        let policy_key = format!("{}:{}", action.namespace, action.tenant);
        let now = Utc::now();

        // 1. Check in-memory cache with a 60-second TTL to ensure we eventually
        //    see updates made on other instances.
        let cached = {
            let map = self.quota_policies.read();
            map.get(&policy_key).cloned()
        };

        const CACHE_TTL_SECS: i64 = 60;

        let policy = if let Some(c) = cached
            && (now - c.cached_at).num_seconds() < CACHE_TTL_SECS
        {
            c.policy
        } else {
            // Cold path: fetch from state store. We fail-open if the store
            // is down to protect system availability.
            let found = match self
                .load_quota_from_state_store(&action.namespace, &action.tenant)
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    warn!(error = %e, "quota policy lookup failed (fail-open)");
                    return Ok(None);
                }
            };

            match found {
                Some(p) => {
                    let cached = CachedPolicy {
                        policy: p.clone(),
                        cached_at: now,
                    };
                    self.quota_policies
                        .write()
                        .insert(policy_key.clone(), cached);
                    p
                }
                None => return Ok(None),
            }
        };

        if !policy.enabled {
            return Ok(None);
        }

        let counter_id =
            acteon_core::quota_counter_key(&action.namespace, &action.tenant, &policy.window, &now);
        let counter_key = acteon_state::StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            acteon_state::KeyKind::QuotaUsage,
            &counter_id,
        );

        let window_ttl = Some(std::time::Duration::from_secs(
            policy.window.duration_seconds(),
        ));

        // 2. Increment usage counter. Fail-open on state store errors.
        let new_count = match self.state.increment(&counter_key, 1, window_ttl).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "quota increment failed (fail-open)");
                return Ok(None);
            }
        };

        #[allow(clippy::cast_sign_loss)]
        let used = new_count as u64;

        if used <= policy.max_actions {
            return Ok(None);
        }

        // Quota exceeded — apply behavior.
        self.apply_overage_behavior(action, &policy, used, &counter_key, window_ttl)
            .await
    }

    /// Apply the configured overage behavior when a tenant exceeds their quota.
    ///
    /// Separated from [`check_quota`](Self::check_quota) to keep each method
    /// under the clippy line-count limit.
    async fn apply_overage_behavior(
        &self,
        action: &Action,
        policy: &acteon_core::QuotaPolicy,
        used: u64,
        counter_key: &acteon_state::StateKey,
        window_ttl: Option<std::time::Duration>,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        match &policy.overage_behavior {
            acteon_core::OverageBehavior::Block => {
                self.metrics.increment_quota_exceeded();
                // Roll back the increment so the blocked request doesn't
                // consume a slot.
                let _ = self.state.increment(counter_key, -1, window_ttl).await;
                info!(
                    tenant = %action.tenant,
                    limit = policy.max_actions,
                    used,
                    "quota exceeded — blocking action"
                );
                Ok(Some(ActionOutcome::QuotaExceeded {
                    tenant: action.tenant.to_string(),
                    limit: policy.max_actions,
                    used,
                    overage_behavior: "block".into(),
                }))
            }
            acteon_core::OverageBehavior::Warn => {
                self.metrics.increment_quota_warned();
                warn!(
                    tenant = %action.tenant,
                    limit = policy.max_actions,
                    used,
                    "quota exceeded — warning, allowing action"
                );
                Ok(None)
            }
            acteon_core::OverageBehavior::Degrade { fallback_provider } => {
                self.metrics.increment_quota_degraded();
                info!(
                    tenant = %action.tenant,
                    fallback = %fallback_provider,
                    "quota exceeded — degrading to fallback provider"
                );
                Ok(Some(ActionOutcome::QuotaExceeded {
                    tenant: action.tenant.to_string(),
                    limit: policy.max_actions,
                    used,
                    overage_behavior: format!("degrade:{fallback_provider}"),
                }))
            }
            acteon_core::OverageBehavior::Notify { target } => {
                self.metrics.increment_quota_notified();
                warn!(
                    tenant = %action.tenant,
                    target = %target,
                    "quota exceeded — notifying admin, allowing action"
                );
                Ok(None)
            }
        }
    }

    /// Try to load a quota policy for `namespace:tenant` from the state store.
    ///
    /// This is the cold-path fallback used by [`check_quota`](Self::check_quota)
    /// when no in-memory policy is found, enabling cross-instance visibility
    /// without requiring a restart.
    ///
    /// Uses a two-step O(1) lookup via the `idx:{namespace}:{tenant}` index
    /// key written by the API layer, avoiding a full `scan_keys_by_kind`.
    async fn load_quota_from_state_store(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<acteon_core::QuotaPolicy>, GatewayError> {
        // Step 1: look up the index key to get the policy ID.
        let idx_suffix = format!("idx:{namespace}:{tenant}");
        let idx_key = acteon_state::StateKey::new(
            "_system",
            "_quotas",
            acteon_state::KeyKind::Quota,
            &idx_suffix,
        );
        let Some(policy_id) = self.state.get(&idx_key).await? else {
            return Ok(None);
        };

        // Step 2: look up the policy by ID.
        let policy_key = acteon_state::StateKey::new(
            "_system",
            "_quotas",
            acteon_state::KeyKind::Quota,
            &policy_id,
        );
        match self.state.get(&policy_key).await? {
            Some(data) => {
                let policy = serde_json::from_str::<acteon_core::QuotaPolicy>(&data)
                    .map_err(|e| GatewayError::Configuration(e.to_string()))?;
                Ok(Some(policy))
            }
            None => Ok(None),
        }
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

    /// Walk the fallback chain for an open circuit, returning either the name
    /// of a healthy fallback provider or the list of fallbacks that were tried.
    async fn resolve_fallback_chain<'a>(
        &self,
        registry: &'a CircuitBreakerRegistry,
        start: &'a str,
    ) -> Result<&'a str, Vec<String>> {
        let mut fallback_chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        visited.insert(start);
        let mut current_name: &str = start;

        loop {
            let next_fallback = registry
                .get(current_name)
                .and_then(|cb| cb.config().fallback_provider.as_deref());

            let Some(fallback_name) = next_fallback else {
                break;
            };

            // Cycle detection (defense-in-depth; builder also validates).
            if !visited.insert(fallback_name) {
                break;
            }

            fallback_chain.push(fallback_name.to_string());

            if self.providers.get(fallback_name).is_none() {
                break;
            }

            // Check the fallback's circuit breaker.
            if let Some(fallback_cb) = registry.get(fallback_name) {
                let (fb_state, fb_transition) = fallback_cb.try_acquire_permit().await;
                if let Some((_from, _to)) = fb_transition {
                    self.metrics.increment_circuit_transitions();
                }
                if fb_state == crate::circuit_breaker::CircuitState::Open {
                    current_name = fallback_name;
                    continue;
                }
            }

            // Fallback is available (no CB or CB is closed/half-open).
            return Ok(fallback_name);
        }

        Err(fallback_chain)
    }

    /// Execute an action on a fallback provider and record the result.
    async fn execute_on_fallback(
        &self,
        action: &Action,
        registry: &CircuitBreakerRegistry,
        target_name: &str,
    ) -> ActionOutcome {
        let target = self
            .providers
            .get(target_name)
            .expect("fallback provider existence checked in resolve_fallback_chain");

        debug!(
            provider = %action.provider,
            fallback = %target_name,
            "circuit open, rerouting to fallback provider"
        );
        let result = self.executor.execute(action, target.as_ref()).await;

        // Record result in the fallback provider's circuit breaker.
        if let Some(fallback_cb) = registry.get(target_name) {
            let fb_transition = match &result {
                ActionOutcome::Executed(_) => fallback_cb.record_success().await,
                ActionOutcome::Failed(err) if err.retryable => fallback_cb.record_failure().await,
                _ => None,
            };
            if fb_transition.is_some() {
                self.metrics.increment_circuit_transitions();
            }
        }

        self.metrics.increment_circuit_fallbacks();
        match result {
            ActionOutcome::Executed(ref resp) => ActionOutcome::Rerouted {
                original_provider: action.provider.to_string(),
                new_provider: target_name.to_string(),
                response: resp.clone(),
            },
            other => other,
        }
    }

    /// Look up the action's provider and execute through the executor.
    ///
    /// When a circuit breaker is configured for the provider and the circuit
    /// is open, the request is rejected immediately. If a fallback provider is
    /// configured, the gateway walks the fallback chain recursively until it
    /// finds a healthy provider or exhausts the chain.
    #[instrument(name = "gateway.execute_action", skip(self, action), fields(provider = %action.provider))]
    async fn execute_action(&self, action: &Action) -> ActionOutcome {
        // Check circuit breaker before executing — walk the fallback chain.
        if let Some(ref registry) = self.circuit_breakers
            && let Some(cb) = registry.get(action.provider.as_str())
        {
            let (state, transition) = cb.try_acquire_permit().await;
            if let Some((_from, _to)) = transition {
                self.metrics.increment_circuit_transitions();
            }
            if state == crate::circuit_breaker::CircuitState::Open {
                match self
                    .resolve_fallback_chain(registry, action.provider.as_str())
                    .await
                {
                    Ok(target_name) => {
                        return self
                            .execute_on_fallback(action, registry, target_name)
                            .await;
                    }
                    Err(fallback_chain) => {
                        self.metrics.increment_circuit_open();
                        return ActionOutcome::CircuitOpen {
                            provider: action.provider.to_string(),
                            fallback_chain,
                        };
                    }
                }
            }
        }

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

        // Record result in circuit breaker.
        // Only retryable failures indicate provider health issues;
        // non-retryable errors (400, 401, 403) are client errors that
        // should not trip the circuit.
        if let Some(ref registry) = self.circuit_breakers
            && let Some(cb) = registry.get(action.provider.as_str())
        {
            let transition = match &result {
                ActionOutcome::Executed(_) => cb.record_success().await,
                ActionOutcome::Failed(err) if err.retryable => cb.record_failure().await,
                _ => None,
            };
            if transition.is_some() {
                self.metrics.increment_circuit_transitions();
            }
        }

        match &result {
            ActionOutcome::Executed(_) => self.metrics.increment_executed(),
            ActionOutcome::Failed(_) => self.metrics.increment_failed(),
            _ => {}
        }
        result
    }

    /// Handle the deduplication verdict: check state, execute only if new.
    #[instrument(name = "gateway.handle_dedup", skip(self, action))]
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
    #[instrument(name = "gateway.handle_reroute", skip(self, action), fields(%target_provider))]
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

    /// Handle the state machine verdict: track event lifecycle.
    #[allow(clippy::too_many_lines)]
    #[instrument(name = "gateway.handle_state_machine", skip_all)]
    async fn handle_state_machine(
        &self,
        action: &Action,
        state_machine_name: &str,
        fingerprint_fields: &[String],
    ) -> Result<ActionOutcome, GatewayError> {
        let state_machine = self.state_machines.get(state_machine_name).ok_or_else(|| {
            GatewayError::Configuration(format!("state machine not found: {state_machine_name}"))
        })?;

        // Compute fingerprint from action fields
        let fingerprint = if let Some(fp) = &action.fingerprint {
            fp.clone()
        } else {
            compute_fingerprint(action, fingerprint_fields)
        };

        // Acquire a lock on the fingerprint to prevent race conditions
        // between different actions affecting the same entity
        let lock_name = format!(
            "state:{}:{}:{}",
            action.namespace, action.tenant, fingerprint
        );
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        // Get current state from state store
        let state_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::EventState,
            &fingerprint,
        );

        let current_state = match self.state.get(&state_key).await? {
            Some(val) => {
                // Parse stored state JSON
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&val) {
                    parsed
                        .get("state")
                        .and_then(|s| s.as_str())
                        .unwrap_or(&state_machine.initial_state)
                        .to_string()
                } else {
                    val
                }
            }
            None => state_machine.initial_state.clone(),
        };

        // Determine the target state from the action's status or use current state
        let target_state = action
            .status
            .clone()
            .unwrap_or_else(|| current_state.clone());

        // Validate state transition
        let (new_state, notify) = if current_state == target_state {
            // No transition needed
            (current_state.clone(), false)
        } else if state_machine.is_transition_allowed(&current_state, &target_state) {
            let transition = state_machine.get_transition(&current_state, &target_state);
            let should_notify = transition.is_some_and(|t| t.on_transition.notify);
            (target_state, should_notify)
        } else {
            debug!(
                from = %current_state,
                to = %target_state,
                "invalid state transition, keeping current state"
            );
            (current_state.clone(), false)
        };

        // Store updated state
        let state_value = serde_json::json!({
            "state": &new_state,
            "fingerprint": &fingerprint,
            "updated_at": Utc::now().to_rfc3339(),
            "action_type": &action.action_type,
        });
        self.state
            .set(&state_key, &state_value.to_string(), None)
            .await?;

        // Update active events index for inhibition lookups
        let active_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::ActiveEvents,
            &action.action_type,
        );
        let active_value = serde_json::json!({
            "state": &new_state,
            "fingerprint": &fingerprint,
        });
        self.state
            .set(&active_key, &active_value.to_string(), None)
            .await?;

        // Create timeout entry if the new state has a configured timeout
        if let Some(timeout_config) = state_machine.get_timeout_for_state(&new_state) {
            #[allow(clippy::cast_possible_wrap)]
            let expires_at =
                Utc::now() + chrono::Duration::seconds(timeout_config.after_seconds as i64);
            let timeout_key = StateKey::new(
                action.namespace.as_str(),
                action.tenant.as_str(),
                KeyKind::EventTimeout,
                &fingerprint,
            );
            let timeout_value = serde_json::json!({
                "fingerprint": &fingerprint,
                "state_machine": state_machine_name,
                "current_state": &new_state,
                "transition_to": &timeout_config.transition_to,
                "expires_at": expires_at.to_rfc3339(),
                "created_at": Utc::now().to_rfc3339(),
                "trace_context": &action.trace_context,
            });
            self.state
                .set(&timeout_key, &timeout_value.to_string(), None)
                .await?;

            // Add to timeout index for efficient O(log N) queries
            self.state
                .index_timeout(&timeout_key, expires_at.timestamp_millis())
                .await?;

            debug!(
                fingerprint = %fingerprint,
                state = %new_state,
                timeout_seconds = timeout_config.after_seconds,
                "created timeout entry"
            );
        } else {
            // Clear any existing timeout if the new state has no timeout
            let timeout_key = StateKey::new(
                action.namespace.as_str(),
                action.tenant.as_str(),
                KeyKind::EventTimeout,
                &fingerprint,
            );
            let _ = self.state.delete(&timeout_key).await;
            // Also remove from index (ignore errors if not present)
            let _ = self.state.remove_timeout_index(&timeout_key).await;
        }

        // Release the fingerprint lock
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        Ok(ActionOutcome::StateChanged {
            fingerprint,
            previous_state: current_state,
            new_state,
            notify,
        })
    }

    /// Compute the HMAC-SHA256 signature for an approval using a specific key.
    ///
    /// The message uses length-prefixed fields to prevent canonicalization
    /// attacks (e.g., `ns="a:b", tenant="c"` vs `ns="a", tenant="b:c"`).
    /// The `expires_at` timestamp binds the signature to a specific expiry
    /// window so leaked links cannot be replayed after expiration.
    fn compute_approval_sig_with_key(
        key: &ApprovalKey,
        ns: &str,
        tenant: &str,
        id: &str,
        expires_at: i64,
    ) -> String {
        let msg = format!(
            "{}:{}\n{}:{}\n{}:{}\n{}",
            ns.len(),
            ns,
            tenant.len(),
            tenant,
            id.len(),
            id,
            expires_at,
        );
        let mut mac = HmacSha256::new_from_slice(&key.secret).expect("HMAC accepts any key size");
        mac.update(msg.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Compute the HMAC-SHA256 signature using the current signing key.
    ///
    /// Returns `(signature, kid)` so that the kid can be appended to URLs.
    fn compute_approval_sig(
        &self,
        ns: &str,
        tenant: &str,
        id: &str,
        expires_at: i64,
    ) -> (String, String) {
        let current = self.approval_keys.current();
        let sig = Self::compute_approval_sig_with_key(current, ns, tenant, id, expires_at);
        (sig, current.kid.clone())
    }

    /// Verify the HMAC-SHA256 signature for an approval.
    ///
    /// If `kid` is `Some`, only the matching key is tried. If `kid` is `None`,
    /// all keys are tried in order for backward compatibility with URLs
    /// generated before key rotation was introduced.
    fn verify_approval_sig(
        &self,
        ns: &str,
        tenant: &str,
        id: &str,
        expires_at: i64,
        sig: &str,
        kid: Option<&str>,
    ) -> bool {
        let keys_to_try: Vec<&ApprovalKey> = if let Some(kid) = kid {
            match self.approval_keys.get(kid) {
                Some(key) => vec![key],
                None => return false,
            }
        } else {
            self.approval_keys.all().iter().collect()
        };

        for key in keys_to_try {
            let expected = Self::compute_approval_sig_with_key(key, ns, tenant, id, expires_at);
            // Constant-time comparison
            let is_match = expected.len() == sig.len()
                && expected
                    .bytes()
                    .zip(sig.bytes())
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0;
            if is_match {
                return true;
            }
        }
        false
    }

    /// Handle the request approval verdict: store approval record, send notification, return pending.
    #[allow(clippy::too_many_lines)]
    #[instrument(name = "gateway.handle_request_approval", skip_all, fields(%rule, %notify_provider))]
    async fn handle_request_approval(
        &self,
        action: &Action,
        rule: &str,
        notify_provider: &str,
        timeout_seconds: u64,
        message: Option<&str>,
    ) -> Result<ActionOutcome, GatewayError> {
        // Generate a UUID as the approval ID
        let id = uuid::Uuid::new_v4().to_string();

        let now = Utc::now();
        #[allow(clippy::cast_possible_wrap)]
        let expires_at = now + chrono::Duration::seconds(timeout_seconds as i64);
        let ttl = Some(Duration::from_secs(timeout_seconds));

        // Compute HMAC signature (includes expires_at to bind sig to this TTL)
        let expires_ts = expires_at.timestamp();
        let (sig, kid) = self.compute_approval_sig(
            action.namespace.as_str(),
            action.tenant.as_str(),
            &id,
            expires_ts,
        );

        // Build the approval record
        let record = ApprovalRecord {
            action: action.clone(),
            token: id.clone(),
            rule: rule.to_owned(),
            created_at: now,
            expires_at,
            status: "pending".to_string(),
            decided_by: None,
            decided_at: None,
            message: message.map(String::from),
            notification_sent: false, // updated below
        };

        let record_json = serde_json::to_string(&record).map_err(|e| {
            GatewayError::Configuration(format!("failed to serialize approval: {e}"))
        })?;
        let record_encrypted = self.encrypt_state_value(&record_json)?;

        // Store the approval record keyed by namespace:tenant:approval:id
        let approval_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::Approval,
            &id,
        );
        self.state
            .set(&approval_key, &record_encrypted, ttl)
            .await?;

        // Store pending approvals index by action ID
        let pending_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::PendingApprovals,
            action.id.as_str(),
        );
        let pending_val = serde_json::json!({
            "token": &id,
            "created_at": now.to_rfc3339(),
            "expires_at": expires_at.to_rfc3339(),
        });
        self.state
            .set(&pending_key, &pending_val.to_string(), ttl)
            .await?;

        // Build HMAC-signed URLs with namespace/tenant in the path
        let external_url = self.external_url.as_deref().unwrap_or_else(|| {
            warn!("`external_url` not configured, using http://localhost:8080");
            "http://localhost:8080"
        });

        if !external_url.starts_with("https://") {
            warn!(url = %external_url, "external_url is not HTTPS - approval links will not be secure");
        }

        let ns = &action.namespace;
        let tenant = &action.tenant;
        let approve_url = format!(
            "{external_url}/v1/approvals/{ns}/{tenant}/{id}/approve?sig={sig}&expires_at={expires_ts}&kid={kid}"
        );
        let reject_url = format!(
            "{external_url}/v1/approvals/{ns}/{tenant}/{id}/reject?sig={sig}&expires_at={expires_ts}&kid={kid}"
        );

        let notification_payload = serde_json::json!({
            "subject": format!("Approval Required: {}", action.action_type),
            "body": format!(
                "Action '{}' requires approval.\n\nReason: {}\n\nApprove: {}\nReject: {}",
                action.action_type,
                message.unwrap_or("Approval required"),
                approve_url,
                reject_url,
            ),
            "approval_url": &approve_url,
            "reject_url": &reject_url,
            "action_id": action.id.to_string(),
            "action_type": &action.action_type,
            "namespace": action.namespace.to_string(),
            "tenant": action.tenant.to_string(),
            "expires_at": expires_at.to_rfc3339(),
        });

        // Execute notification directly via provider (bypass rules)
        let mut notification_sent = false;
        if let Some(provider) = self.providers.get(notify_provider) {
            let notification = Action::new(
                action.namespace.as_str(),
                action.tenant.as_str(),
                notify_provider,
                "approval_notification",
                notification_payload,
            );
            let result = self
                .executor
                .execute(&notification, provider.as_ref())
                .await;
            if let ActionOutcome::Failed(err) = &result {
                error!(
                    error = %err.message,
                    "approval notification failed, approval is pending but human may not receive the link"
                );
            } else {
                notification_sent = true;
                info!("approval notification sent via {notify_provider}");
            }
        } else {
            error!(
                provider = %notify_provider,
                "notification provider not found, approval is pending but human will not receive the link"
            );
        }

        // Update the record with notification status
        if notification_sent {
            let mut updated = record;
            updated.notification_sent = true;
            let updated_json = serde_json::to_string(&updated).map_err(|e| {
                GatewayError::Configuration(format!("failed to serialize approval: {e}"))
            })?;
            self.state.set(&approval_key, &updated_json, ttl).await?;
        }

        self.metrics.increment_pending_approval();

        Ok(ActionOutcome::PendingApproval {
            approval_id: id,
            expires_at,
            approve_url,
            reject_url,
            notification_sent,
        })
    }

    /// Handle the group verdict: add event to group for batched notification.
    #[instrument(name = "gateway.handle_group", skip_all)]
    async fn handle_group(
        &self,
        action: &Action,
        group_by: &[String],
        group_wait_seconds: u64,
        _group_interval_seconds: u64,
        _max_group_size: usize,
    ) -> Result<ActionOutcome, GatewayError> {
        let (group_id, group_key, group_size, notify_at) = self
            .group_manager
            .add_to_group(action, group_by, group_wait_seconds, self.state.as_ref())
            .await?;

        self.emit_stream_event(StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: Utc::now(),
            event_type: StreamEventType::GroupEventAdded {
                group_id: group_id.clone(),
                group_key: group_key.clone(),
                event_count: group_size,
            },
            namespace: action.namespace.to_string(),
            tenant: action.tenant.to_string(),
            action_type: Some(action.action_type.clone()),
            action_id: Some(action.id.to_string()),
        });

        Ok(ActionOutcome::Grouped {
            group_id,
            group_size,
            notify_at,
        })
    }

    /// Maximum allowed delay for scheduled actions (7 days).
    ///
    /// Prevents unbounded state store growth from actions scheduled far into the
    /// future. Rules should use reasonable delays; anything longer than a week is
    /// likely a misconfiguration.
    const MAX_SCHEDULE_DELAY_SECONDS: u64 = 7 * 24 * 60 * 60;

    /// Grace period added to the TTL of stored scheduled action data.
    ///
    /// The data TTL is `delay_seconds + GRACE_SECONDS` so that the background
    /// processor has time to pick it up even if the processor is down for an
    /// extended period. Set to 24 hours to survive longer outages. If the data
    /// expires before dispatch, the action is silently dropped (preferable to
    /// permanent orphaned state).
    const SCHEDULE_GRACE_SECONDS: u64 = 86_400;

    /// Handle the schedule verdict: store the action for delayed execution.
    ///
    /// The action is persisted in the state store with a `ScheduledAction` key
    /// and indexed in the `PendingScheduled` index for efficient polling by the
    /// background processor. A TTL is set on the stored data so that orphaned
    /// entries are automatically cleaned up.
    #[instrument(
        name = "gateway.handle_schedule",
        skip(self, action),
        fields(delay_seconds)
    )]
    async fn handle_schedule(
        &self,
        action: &Action,
        delay_seconds: u64,
    ) -> Result<ActionOutcome, GatewayError> {
        // Validate delay bounds.
        if delay_seconds == 0 {
            return Err(GatewayError::Configuration(
                "scheduled delay must be at least 1 second".to_string(),
            ));
        }
        if delay_seconds > Self::MAX_SCHEDULE_DELAY_SECONDS {
            return Err(GatewayError::Configuration(format!(
                "scheduled delay {delay_seconds}s exceeds maximum of {}s (7 days)",
                Self::MAX_SCHEDULE_DELAY_SECONDS
            )));
        }

        // Reject re-scheduling of already-scheduled actions to prevent infinite loops.
        if action
            .payload
            .get("_scheduled_dispatch")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            warn!(
                action_id = %action.id,
                "rejecting re-schedule of already-scheduled action"
            );
            return Err(GatewayError::Configuration(
                "cannot re-schedule an already-scheduled action".to_string(),
            ));
        }

        let now = Utc::now();
        #[allow(clippy::cast_possible_wrap)]
        let scheduled_for = now + chrono::Duration::seconds(delay_seconds as i64);
        let action_id = uuid::Uuid::new_v4().to_string();

        // TTL = delay + grace period so orphaned entries self-clean.
        let ttl = Some(Duration::from_secs(
            delay_seconds + Self::SCHEDULE_GRACE_SECONDS,
        ));

        // Persist the scheduled action data.
        let sched_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::ScheduledAction,
            &action_id,
        );
        let sched_data = serde_json::json!({
            "action_id": &action_id,
            "action": action,
            "scheduled_for": scheduled_for.to_rfc3339(),
            "created_at": now.to_rfc3339(),
        });
        let sched_value = self.encrypt_state_value(&sched_data.to_string())?;
        self.state.set(&sched_key, &sched_value, ttl).await?;

        // Add to pending scheduled index using the timeout index mechanism.
        let pending_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::PendingScheduled,
            &action_id,
        );
        self.state
            .set(
                &pending_key,
                &scheduled_for.timestamp_millis().to_string(),
                ttl,
            )
            .await?;
        self.state
            .index_timeout(&pending_key, scheduled_for.timestamp_millis())
            .await?;

        self.metrics.increment_scheduled();

        info!(
            action_id = %action_id,
            scheduled_for = %scheduled_for,
            delay_seconds = delay_seconds,
            "action scheduled for delayed execution"
        );

        Ok(ActionOutcome::Scheduled {
            action_id,
            scheduled_for,
        })
    }

    /// Handle the chain verdict: create chain state and start async execution.
    #[instrument(name = "gateway.handle_chain", skip(self, action), fields(%chain_name))]
    async fn handle_chain(
        &self,
        action: &Action,
        chain_name: &str,
    ) -> Result<ActionOutcome, GatewayError> {
        let chain_config = self.chains.get(chain_name).ok_or_else(|| {
            GatewayError::ChainError(format!("chain configuration not found: {chain_name}"))
        })?;

        if chain_config.steps.is_empty() {
            return Err(GatewayError::ChainError(format!(
                "chain '{chain_name}' has no steps"
            )));
        }

        let chain_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let total_steps = chain_config.steps.len();
        let first_step = chain_config.steps[0].name.clone();

        #[allow(clippy::cast_possible_wrap)]
        let expires_at = chain_config
            .timeout_seconds
            .map(|secs| now + chrono::Duration::seconds(secs as i64));

        // Validate chain configuration if it uses branching.
        if chain_config.has_branches() {
            let validation_errors = chain_config.validate();
            if !validation_errors.is_empty() {
                return Err(GatewayError::ChainError(format!(
                    "invalid chain configuration '{}': {}",
                    chain_name,
                    validation_errors.join("; ")
                )));
            }
        }

        let chain_state = ChainState {
            chain_id: chain_id.clone(),
            chain_name: chain_name.to_owned(),
            origin_action: action.clone(),
            current_step: 0,
            total_steps,
            status: ChainStatus::Running,
            step_results: vec![None; total_steps],
            started_at: now,
            updated_at: now,
            expires_at,
            namespace: action.namespace.to_string(),
            tenant: action.tenant.to_string(),
            cancel_reason: None,
            cancelled_by: None,
            execution_path: vec![first_step.clone()],
        };

        // Persist chain state.
        let chain_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::Chain,
            &chain_id,
        );
        let state_json = serde_json::to_string(&chain_state).map_err(|e| {
            GatewayError::ChainError(format!("failed to serialize chain state: {e}"))
        })?;
        let state_encrypted = self.encrypt_state_value(&state_json)?;
        self.state.set(&chain_key, &state_encrypted, None).await?;

        // Add to pending chains index.
        let pending_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::PendingChains,
            &chain_id,
        );
        let pending_val = serde_json::json!({
            "chain_id": &chain_id,
            "chain_name": chain_name,
            "started_at": now.to_rfc3339(),
        });
        self.state
            .set(&pending_key, &pending_val.to_string(), None)
            .await?;
        let ready_at = chain_config.steps[0]
            .delay_seconds
            .map_or(0, |d| now.timestamp_millis() + (d.cast_signed() * 1000));
        self.state.index_chain_ready(&pending_key, ready_at).await?;

        self.metrics.increment_chains_started();

        info!(
            chain_id = %chain_id,
            chain_name = %chain_name,
            total_steps = total_steps,
            "chain execution started"
        );

        Ok(ActionOutcome::ChainStarted {
            chain_id,
            chain_name: chain_name.to_owned(),
            total_steps,
            first_step,
        })
    }

    /// Advance a chain execution by running the next pending step.
    ///
    /// This method is called by the background processor to resume chain
    /// execution after the initial dispatch or a crash.
    #[allow(clippy::too_many_lines)]
    #[instrument(name = "gateway.advance_chain", skip(self), fields(%namespace, %tenant, %chain_id))]
    pub async fn advance_chain(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
    ) -> Result<(), GatewayError> {
        let chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, chain_id);

        // Acquire a lock to prevent concurrent advancement.
        let lock_name = format!("chain:{chain_id}");
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(60), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        // Load current chain state.
        let state_raw = self.state.get(&chain_key).await?.ok_or_else(|| {
            GatewayError::ChainError(format!("chain state not found: {chain_id}"))
        })?;
        let state_json = self.decrypt_state_value(&state_raw)?;
        let mut chain_state: ChainState = serde_json::from_str(&state_json).map_err(|e| {
            GatewayError::ChainError(format!("failed to deserialize chain state: {e}"))
        })?;

        // Remove from the ready index to prevent double-scheduling.
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingChains, chain_id);
        self.state.remove_chain_ready_index(&pending_key).await?;

        // Check if chain is still running.
        if chain_state.status != ChainStatus::Running {
            guard
                .release()
                .await
                .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
            return Ok(());
        }

        // Check timeout.
        if let Some(expires_at) = chain_state.expires_at
            && Utc::now() >= expires_at
        {
            chain_state.status = ChainStatus::TimedOut;
            chain_state.updated_at = Utc::now();
            self.persist_chain_state(&chain_key, &chain_state, self.completed_chain_ttl)
                .await?;
            self.cleanup_pending_chain(namespace, tenant, chain_id)
                .await?;
            self.metrics.increment_chains_failed();
            self.emit_chain_terminal_audit(&chain_state, "chain_timed_out");
            self.emit_stream_event(StreamEvent {
                id: uuid::Uuid::now_v7().to_string(),
                timestamp: Utc::now(),
                event_type: StreamEventType::ChainCompleted {
                    chain_id: chain_id.to_string(),
                    status: "timed_out".to_string(),
                    execution_path: chain_state.execution_path.clone(),
                },
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                action_type: Some(chain_state.chain_name.clone()),
                action_id: Some(chain_state.origin_action.id.to_string()),
            });
            guard
                .release()
                .await
                .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
            warn!(chain_id = %chain_id, "chain timed out");
            return Ok(());
        }

        let chain_config = self.chains.get(&chain_state.chain_name).ok_or_else(|| {
            GatewayError::ChainError(format!(
                "chain configuration not found: {}",
                chain_state.chain_name
            ))
        })?;

        // Use the pre-computed step index map (built once at gateway construction).
        let empty_index_map = HashMap::new();
        let step_index_map = self
            .chain_step_indices
            .get(&chain_state.chain_name)
            .unwrap_or(&empty_index_map);

        let step_idx = chain_state.current_step;
        let step_config = &chain_config.steps[step_idx];

        // Resolve the payload template.
        let payload = crate::chain::resolve_template(
            &step_config.payload_template,
            &chain_state.origin_action,
            &chain_state.step_results,
            &chain_config.steps,
            chain_id,
            step_idx,
            &chain_state.execution_path,
        );

        // Build and execute the synthetic action.
        let mut step_action = Action::new(
            namespace,
            tenant,
            step_config.provider.as_str(),
            &step_config.action_type,
            payload,
        );

        // Idempotency: ensure this step is not executed twice.
        // Use step name in the dedup key to handle branching chains where
        // step indices may not be sequential.
        let step_dedup_key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Dedup,
            format!("chain-step:{chain_id}:{}", step_config.name),
        );
        let dedup_ttl = chain_state.expires_at.map_or(
            Duration::from_secs(86400), // 24h default
            |ea| {
                let remaining = ea - Utc::now();
                Duration::from_secs(remaining.num_seconds().max(1).cast_unsigned())
            },
        );
        let is_new = self
            .state
            .check_and_set(&step_dedup_key, "dispatched", Some(dedup_ttl))
            .await?;

        if !is_new {
            // Step was previously dispatched. Reload chain state to check progress.
            let already_advanced = if let Some(json) = self.state.get(&chain_key).await? {
                serde_json::from_str::<ChainState>(&json)
                    .map(|fresh| {
                        // For branching chains, check if the step result is already
                        // recorded. For linear chains, the index check is sufficient.
                        fresh.step_results[step_idx].is_some() || fresh.current_step != step_idx
                    })
                    .unwrap_or(false)
            } else {
                false
            };

            if already_advanced {
                // Result was persisted; skip gracefully.
                debug!(
                    chain_id = %chain_id,
                    step_idx = step_idx,
                    "step already completed, skipping"
                );
                guard
                    .release()
                    .await
                    .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
                return Ok(());
            }

            // Crash between execute and persist: mark as interrupted failure.
            warn!(
                chain_id = %chain_id,
                step_idx = step_idx,
                "step previously dispatched but not persisted, marking as failed"
            );
            chain_state.step_results[step_idx] = Some(StepResult {
                step_name: step_config.name.clone(),
                success: false,
                response_body: None,
                error: Some("step interrupted (duplicate dispatch detected)".to_string()),
                completed_at: Utc::now(),
            });
            chain_state.status = ChainStatus::Failed;
            chain_state.updated_at = Utc::now();
            self.persist_chain_state(&chain_key, &chain_state, self.completed_chain_ttl)
                .await?;
            self.cleanup_pending_chain(namespace, tenant, chain_id)
                .await?;
            self.metrics.increment_chains_failed();
            if let Some(ref sr) = chain_state.step_results[step_idx] {
                self.emit_chain_step_audit(
                    &chain_state,
                    step_config,
                    step_idx,
                    "chain_step_failed",
                    sr,
                    Duration::ZERO,
                    None,
                );
            }
            self.emit_chain_terminal_audit(&chain_state, "chain_failed");
            let _ = self.state.delete(&step_dedup_key).await;
            guard
                .release()
                .await
                .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
            return Ok(());
        }

        // Enforce tenant quota for each chain step so chains cannot bypass
        // limits.
        if let Some(quota_outcome) = self.check_quota(&step_action).await? {
            match quota_outcome {
                ActionOutcome::QuotaExceeded {
                    ref overage_behavior,
                    ..
                } if overage_behavior == "block" => {
                    let now = Utc::now();
                    chain_state.step_results[step_idx] = Some(StepResult {
                        step_name: step_config.name.clone(),
                        success: false,
                        response_body: None,
                        error: Some("quota exceeded — chain step blocked".to_string()),
                        completed_at: now,
                    });
                    chain_state.status = ChainStatus::Failed;
                    chain_state.updated_at = now;
                    self.persist_chain_state(&chain_key, &chain_state, self.completed_chain_ttl)
                        .await?;
                    self.cleanup_pending_chain(namespace, tenant, chain_id)
                        .await?;
                    self.metrics.increment_chains_failed();
                    if let Some(ref sr) = chain_state.step_results[step_idx] {
                        self.emit_chain_step_audit(
                            &chain_state,
                            step_config,
                            step_idx,
                            "chain_step_quota_exceeded",
                            sr,
                            Duration::ZERO,
                            None,
                        );
                    }
                    self.emit_chain_terminal_audit(&chain_state, "chain_failed");
                    let _ = self.state.delete(&step_dedup_key).await;
                    guard
                        .release()
                        .await
                        .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
                    return Ok(());
                }
                ActionOutcome::QuotaExceeded {
                    ref overage_behavior,
                    ..
                } if overage_behavior.starts_with("degrade:") => {
                    if let Some(fallback) = overage_behavior.strip_prefix("degrade:") {
                        info!(
                            chain_id = %chain_id,
                            step = %step_config.name,
                            fallback = %fallback,
                            "quota exceeded — degrading chain step to fallback provider"
                        );
                        step_action.provider = fallback.into();
                    }
                }
                _ => {
                    // Warn / Notify are already recorded by check_quota;
                    // proceed with execution on original provider.
                }
            }
        }

        let step_payload = step_action.payload.clone();
        let step_start = std::time::Instant::now();
        let outcome = self.execute_action(&step_action).await;
        let step_duration = step_start.elapsed();
        let now = Utc::now();

        match &outcome {
            ActionOutcome::Executed(resp) => {
                let step_result = StepResult {
                    step_name: step_config.name.clone(),
                    success: true,
                    response_body: Some(resp.body.clone()),
                    error: None,
                    completed_at: now,
                };
                chain_state.step_results[step_idx] = Some(step_result.clone());

                // Determine next step using branch evaluation.
                let next_step_idx =
                    Self::resolve_next_step(chain_config, step_idx, &step_result, step_index_map);

                if let Some(next_idx) = next_step_idx {
                    // Advance to the next step (may be non-sequential for branching).
                    chain_state.current_step = next_idx;
                    chain_state.updated_at = now;
                    chain_state
                        .execution_path
                        .push(chain_config.steps[next_idx].name.clone());
                    self.persist_chain_state(&chain_key, &chain_state, None)
                        .await?;
                    let ready_at = chain_config.steps[next_idx]
                        .delay_seconds
                        .map_or(0, |d| now.timestamp_millis() + (d.cast_signed() * 1000));
                    self.state.index_chain_ready(&pending_key, ready_at).await?;
                    // step success, more steps
                    self.emit_chain_step_audit(
                        &chain_state,
                        step_config,
                        step_idx,
                        "chain_step_completed",
                        &step_result,
                        step_duration,
                        Some(&step_payload),
                    );
                    self.emit_stream_event(StreamEvent {
                        id: uuid::Uuid::now_v7().to_string(),
                        timestamp: Utc::now(),
                        event_type: StreamEventType::ChainStepCompleted {
                            chain_id: chain_id.to_string(),
                            step_name: step_config.name.clone(),
                            step_index: step_idx,
                            success: true,
                            next_step: Some(chain_config.steps[next_idx].name.clone()),
                        },
                        namespace: namespace.to_string(),
                        tenant: tenant.to_string(),
                        action_type: Some(step_config.action_type.clone()),
                        action_id: Some(chain_state.origin_action.id.to_string()),
                    });
                } else {
                    // Chain completed successfully.
                    chain_state.status = ChainStatus::Completed;
                    chain_state.updated_at = now;
                    self.persist_chain_state(&chain_key, &chain_state, self.completed_chain_ttl)
                        .await?;
                    self.cleanup_pending_chain(namespace, tenant, chain_id)
                        .await?;
                    self.metrics.increment_chains_completed();
                    // step success, chain completed
                    self.emit_chain_step_audit(
                        &chain_state,
                        step_config,
                        step_idx,
                        "chain_step_completed",
                        &step_result,
                        step_duration,
                        Some(&step_payload),
                    );
                    self.emit_chain_terminal_audit(&chain_state, "chain_completed");
                    self.emit_stream_event(StreamEvent {
                        id: uuid::Uuid::now_v7().to_string(),
                        timestamp: Utc::now(),
                        event_type: StreamEventType::ChainStepCompleted {
                            chain_id: chain_id.to_string(),
                            step_name: step_config.name.clone(),
                            step_index: step_idx,
                            success: true,
                            next_step: None,
                        },
                        namespace: namespace.to_string(),
                        tenant: tenant.to_string(),
                        action_type: Some(step_config.action_type.clone()),
                        action_id: Some(chain_state.origin_action.id.to_string()),
                    });
                    self.emit_stream_event(StreamEvent {
                        id: uuid::Uuid::now_v7().to_string(),
                        timestamp: Utc::now(),
                        event_type: StreamEventType::ChainCompleted {
                            chain_id: chain_id.to_string(),
                            status: "completed".to_string(),
                            execution_path: chain_state.execution_path.clone(),
                        },
                        namespace: namespace.to_string(),
                        tenant: tenant.to_string(),
                        action_type: Some(chain_state.chain_name.clone()),
                        action_id: Some(chain_state.origin_action.id.to_string()),
                    });
                    info!(chain_id = %chain_id, "chain completed successfully");
                }
            }
            ActionOutcome::Failed(err) => {
                let step_policy = step_config
                    .on_failure
                    .as_ref()
                    .unwrap_or(&acteon_core::chain::StepFailurePolicy::Abort);

                chain_state.step_results[step_idx] = Some(StepResult {
                    step_name: step_config.name.clone(),
                    success: false,
                    response_body: None,
                    error: Some(err.message.clone()),
                    completed_at: now,
                });

                match step_policy {
                    acteon_core::chain::StepFailurePolicy::Abort => {
                        chain_state.status = ChainStatus::Failed;
                        chain_state.updated_at = now;
                        self.persist_chain_state(
                            &chain_key,
                            &chain_state,
                            self.completed_chain_ttl,
                        )
                        .await?;
                        self.cleanup_pending_chain(namespace, tenant, chain_id)
                            .await?;
                        self.metrics.increment_chains_failed();
                        // #5: step failed, Abort
                        if let Some(ref sr) = chain_state.step_results[step_idx] {
                            self.emit_chain_step_audit(
                                &chain_state,
                                step_config,
                                step_idx,
                                "chain_step_failed",
                                sr,
                                step_duration,
                                Some(&step_payload),
                            );
                        }
                        self.emit_chain_terminal_audit(&chain_state, "chain_failed");
                        self.emit_stream_event(StreamEvent {
                            id: uuid::Uuid::now_v7().to_string(),
                            timestamp: Utc::now(),
                            event_type: StreamEventType::ChainStepCompleted {
                                chain_id: chain_id.to_string(),
                                step_name: step_config.name.clone(),
                                step_index: step_idx,
                                success: false,
                                next_step: None,
                            },
                            namespace: namespace.to_string(),
                            tenant: tenant.to_string(),
                            action_type: Some(step_config.action_type.clone()),
                            action_id: Some(chain_state.origin_action.id.to_string()),
                        });
                        self.emit_stream_event(StreamEvent {
                            id: uuid::Uuid::now_v7().to_string(),
                            timestamp: Utc::now(),
                            event_type: StreamEventType::ChainCompleted {
                                chain_id: chain_id.to_string(),
                                status: "failed".to_string(),
                                execution_path: chain_state.execution_path.clone(),
                            },
                            namespace: namespace.to_string(),
                            tenant: tenant.to_string(),
                            action_type: Some(chain_state.chain_name.clone()),
                            action_id: Some(chain_state.origin_action.id.to_string()),
                        });
                        warn!(
                            chain_id = %chain_id,
                            step = %step_config.name,
                            "chain step failed, aborting"
                        );
                    }
                    acteon_core::chain::StepFailurePolicy::Skip => {
                        // For skip, also evaluate branch conditions (a failed
                        // step may branch to a recovery step).
                        let skip_result = chain_state.step_results[step_idx]
                            .as_ref()
                            .expect("step result was just set");
                        let next_step_idx = Self::resolve_next_step(
                            chain_config,
                            step_idx,
                            skip_result,
                            step_index_map,
                        );

                        if let Some(next_idx) = next_step_idx {
                            chain_state.current_step = next_idx;
                            chain_state.updated_at = now;
                            chain_state
                                .execution_path
                                .push(chain_config.steps[next_idx].name.clone());
                            self.persist_chain_state(&chain_key, &chain_state, None)
                                .await?;
                            let ready_at = chain_config.steps[next_idx]
                                .delay_seconds
                                .map_or(0, |d| now.timestamp_millis() + (d.cast_signed() * 1000));
                            self.state.index_chain_ready(&pending_key, ready_at).await?;
                            // step failed, Skip, more steps
                            self.emit_chain_step_audit(
                                &chain_state,
                                step_config,
                                step_idx,
                                "chain_step_skipped",
                                skip_result,
                                step_duration,
                                Some(&step_payload),
                            );
                            self.emit_stream_event(StreamEvent {
                                id: uuid::Uuid::now_v7().to_string(),
                                timestamp: Utc::now(),
                                event_type: StreamEventType::ChainStepCompleted {
                                    chain_id: chain_id.to_string(),
                                    step_name: step_config.name.clone(),
                                    step_index: step_idx,
                                    success: false,
                                    next_step: Some(chain_config.steps[next_idx].name.clone()),
                                },
                                namespace: namespace.to_string(),
                                tenant: tenant.to_string(),
                                action_type: Some(step_config.action_type.clone()),
                                action_id: Some(chain_state.origin_action.id.to_string()),
                            });
                        } else {
                            chain_state.status = ChainStatus::Completed;
                            chain_state.updated_at = now;
                            self.persist_chain_state(
                                &chain_key,
                                &chain_state,
                                self.completed_chain_ttl,
                            )
                            .await?;
                            self.cleanup_pending_chain(namespace, tenant, chain_id)
                                .await?;
                            self.metrics.increment_chains_completed();
                            // step failed, Skip, chain completed
                            self.emit_chain_step_audit(
                                &chain_state,
                                step_config,
                                step_idx,
                                "chain_step_skipped",
                                skip_result,
                                step_duration,
                                Some(&step_payload),
                            );
                            self.emit_chain_terminal_audit(&chain_state, "chain_completed");
                            self.emit_stream_event(StreamEvent {
                                id: uuid::Uuid::now_v7().to_string(),
                                timestamp: Utc::now(),
                                event_type: StreamEventType::ChainStepCompleted {
                                    chain_id: chain_id.to_string(),
                                    step_name: step_config.name.clone(),
                                    step_index: step_idx,
                                    success: false,
                                    next_step: None,
                                },
                                namespace: namespace.to_string(),
                                tenant: tenant.to_string(),
                                action_type: Some(step_config.action_type.clone()),
                                action_id: Some(chain_state.origin_action.id.to_string()),
                            });
                            self.emit_stream_event(StreamEvent {
                                id: uuid::Uuid::now_v7().to_string(),
                                timestamp: Utc::now(),
                                event_type: StreamEventType::ChainCompleted {
                                    chain_id: chain_id.to_string(),
                                    status: "completed".to_string(),
                                    execution_path: chain_state.execution_path.clone(),
                                },
                                namespace: namespace.to_string(),
                                tenant: tenant.to_string(),
                                action_type: Some(chain_state.chain_name.clone()),
                                action_id: Some(chain_state.origin_action.id.to_string()),
                            });
                        }
                    }
                    acteon_core::chain::StepFailurePolicy::Dlq => {
                        if let Some(ref dlq) = self.dlq {
                            dlq.push(step_action, err.message.clone(), err.attempts)
                                .await;
                        }
                        chain_state.status = ChainStatus::Failed;
                        chain_state.updated_at = now;
                        self.persist_chain_state(
                            &chain_key,
                            &chain_state,
                            self.completed_chain_ttl,
                        )
                        .await?;
                        self.cleanup_pending_chain(namespace, tenant, chain_id)
                            .await?;
                        self.metrics.increment_chains_failed();
                        // #8: step failed, Dlq
                        if let Some(ref sr) = chain_state.step_results[step_idx] {
                            self.emit_chain_step_audit(
                                &chain_state,
                                step_config,
                                step_idx,
                                "chain_step_failed",
                                sr,
                                step_duration,
                                Some(&step_payload),
                            );
                        }
                        self.emit_chain_terminal_audit(&chain_state, "chain_failed");
                        if let Some(ref sr) = chain_state.step_results[step_idx] {
                            self.emit_stream_event(StreamEvent {
                                id: uuid::Uuid::now_v7().to_string(),
                                timestamp: Utc::now(),
                                event_type: StreamEventType::ChainStepCompleted {
                                    chain_id: chain_id.to_string(),
                                    step_name: sr.step_name.clone(),
                                    step_index: step_idx,
                                    success: false,
                                    next_step: None,
                                },
                                namespace: namespace.to_string(),
                                tenant: tenant.to_string(),
                                action_type: Some(step_config.action_type.clone()),
                                action_id: Some(chain_state.origin_action.id.to_string()),
                            });
                        }
                        self.emit_stream_event(StreamEvent {
                            id: uuid::Uuid::now_v7().to_string(),
                            timestamp: Utc::now(),
                            event_type: StreamEventType::ChainCompleted {
                                chain_id: chain_id.to_string(),
                                status: "failed".to_string(),
                                execution_path: chain_state.execution_path.clone(),
                            },
                            namespace: namespace.to_string(),
                            tenant: tenant.to_string(),
                            action_type: Some(chain_state.chain_name.clone()),
                            action_id: Some(chain_state.origin_action.id.to_string()),
                        });
                    }
                }
            }
            _ => {
                // Unexpected outcome — treat as failure.
                chain_state.step_results[step_idx] = Some(StepResult {
                    step_name: step_config.name.clone(),
                    success: false,
                    response_body: None,
                    error: Some(format!("unexpected outcome: {outcome:?}")),
                    completed_at: now,
                });
                chain_state.status = ChainStatus::Failed;
                chain_state.updated_at = now;
                self.persist_chain_state(&chain_key, &chain_state, self.completed_chain_ttl)
                    .await?;
                self.cleanup_pending_chain(namespace, tenant, chain_id)
                    .await?;
                self.metrics.increment_chains_failed();
                // #9: unexpected outcome
                if let Some(ref sr) = chain_state.step_results[step_idx] {
                    self.emit_chain_step_audit(
                        &chain_state,
                        step_config,
                        step_idx,
                        "chain_step_failed",
                        sr,
                        step_duration,
                        Some(&step_payload),
                    );
                }
                self.emit_chain_terminal_audit(&chain_state, "chain_failed");
                self.emit_stream_event(StreamEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    timestamp: Utc::now(),
                    event_type: StreamEventType::ChainStepCompleted {
                        chain_id: chain_id.to_string(),
                        step_name: step_config.name.clone(),
                        step_index: step_idx,
                        success: false,
                        next_step: None,
                    },
                    namespace: namespace.to_string(),
                    tenant: tenant.to_string(),
                    action_type: Some(step_config.action_type.clone()),
                    action_id: Some(chain_state.origin_action.id.to_string()),
                });
                self.emit_stream_event(StreamEvent {
                    id: uuid::Uuid::now_v7().to_string(),
                    timestamp: Utc::now(),
                    event_type: StreamEventType::ChainCompleted {
                        chain_id: chain_id.to_string(),
                        status: "failed".to_string(),
                        execution_path: chain_state.execution_path.clone(),
                    },
                    namespace: namespace.to_string(),
                    tenant: tenant.to_string(),
                    action_type: Some(chain_state.chain_name.clone()),
                    action_id: Some(chain_state.origin_action.id.to_string()),
                });
            }
        }

        // Clean up the dedup key after the step result has been persisted.
        let _ = self.state.delete(&step_dedup_key).await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        Ok(())
    }

    /// Determine the next step index after a step completes, considering branch
    /// conditions on the completed step.
    ///
    /// Returns `Some(index)` for the next step to execute, or `None` if the
    /// chain should complete (no more steps).
    fn resolve_next_step(
        chain_config: &ChainConfig,
        step_idx: usize,
        step_result: &StepResult,
        step_index_map: &HashMap<String, usize>,
    ) -> Option<usize> {
        let step_config = &chain_config.steps[step_idx];

        // If the step has branches, evaluate them.
        if step_config.has_branches() {
            // Evaluate branch conditions in order; first match wins.
            for branch in &step_config.branches {
                if branch.evaluate(step_result) {
                    return step_index_map.get(&branch.target).copied();
                }
            }

            // No branch matched — use default_next if set.
            if let Some(ref default_next) = step_config.default_next {
                return step_index_map.get(default_next).copied();
            }

            // Branches were defined but none matched and no default_next —
            // fall through to sequential advancement.
            debug!(
                step = step_config.name,
                "branch conditions defined but none matched and no default_next; \
                 falling through to sequential advancement"
            );
        }

        // Fall through to sequential advancement.
        let next = step_idx + 1;
        if next < chain_config.steps.len() {
            Some(next)
        } else {
            None
        }
    }

    /// Persist chain state to the state store.
    ///
    /// When `ttl` is `Some`, the record will expire after the given duration.
    /// Use this for terminal chain states (completed, failed, cancelled, timed out)
    /// so they are automatically cleaned up.
    async fn persist_chain_state(
        &self,
        chain_key: &StateKey,
        chain_state: &ChainState,
        ttl: Option<Duration>,
    ) -> Result<(), GatewayError> {
        let json = serde_json::to_string(chain_state).map_err(|e| {
            GatewayError::ChainError(format!("failed to serialize chain state: {e}"))
        })?;
        let encrypted = self.encrypt_state_value(&json)?;
        self.state.set(chain_key, &encrypted, ttl).await?;
        Ok(())
    }

    /// Remove a chain from the pending chains index.
    async fn cleanup_pending_chain(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
    ) -> Result<(), GatewayError> {
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingChains, chain_id);
        let _ = self.state.delete(&pending_key).await;
        let _ = self.state.remove_chain_ready_index(&pending_key).await;
        Ok(())
    }

    /// Emit a step-level audit record for a chain step event.
    #[allow(clippy::too_many_arguments)]
    fn emit_chain_step_audit(
        &self,
        chain_state: &ChainState,
        step_config: &ChainStepConfig,
        step_idx: usize,
        outcome: &str,
        step_result: &StepResult,
        step_duration: std::time::Duration,
        step_payload: Option<&serde_json::Value>,
    ) {
        if let Some(ref audit) = self.audit {
            let mut outcome_details = serde_json::json!({
                "step_name": step_config.name,
                "step_index": step_idx,
                "total_steps": chain_state.total_steps,
            });
            if let Some(ref body) = step_result.response_body {
                outcome_details["response_body"] = body.clone();
            }
            if let Some(ref err) = step_result.error {
                outcome_details["error"] = serde_json::Value::String(err.clone());
            }

            #[allow(clippy::cast_possible_truncation)]
            let dispatched_at = step_result.completed_at
                - chrono::Duration::milliseconds(step_duration.as_millis() as i64);

            #[allow(clippy::cast_possible_wrap)]
            let expires_at = self
                .audit_ttl_seconds
                .map(|secs| dispatched_at + chrono::Duration::seconds(secs as i64));

            let action_payload = if self.audit_store_payload {
                step_payload.cloned()
            } else {
                None
            };

            let record = AuditRecord {
                id: uuid::Uuid::now_v7().to_string(),
                action_id: chain_state.origin_action.id.to_string(),
                chain_id: Some(chain_state.chain_id.clone()),
                namespace: chain_state.namespace.clone(),
                tenant: chain_state.tenant.clone(),
                provider: step_config.provider.clone(),
                action_type: step_config.action_type.clone(),
                verdict: "chain".to_owned(),
                matched_rule: None,
                outcome: outcome.to_owned(),
                action_payload,
                verdict_details: serde_json::json!({}),
                outcome_details,
                metadata: enrich_audit_metadata(&chain_state.origin_action),
                dispatched_at,
                completed_at: step_result.completed_at,
                duration_ms: u64::try_from(step_duration.as_millis()).unwrap_or(u64::MAX),
                expires_at,
                caller_id: String::new(),
                auth_method: String::new(),
            };

            let audit = Arc::clone(audit);
            self.audit_tracker.spawn(async move {
                if let Err(e) = audit.record(record).await {
                    warn!(error = %e, "chain step audit recording failed");
                }
            });
        }
    }

    /// Emit a terminal summary audit record for a chain lifecycle event.
    fn emit_chain_terminal_audit(&self, chain_state: &ChainState, outcome: &str) {
        if let Some(ref audit) = self.audit {
            let now = Utc::now();

            let step_results_json: Vec<serde_json::Value> = chain_state
                .step_results
                .iter()
                .map(|sr| match sr {
                    Some(r) => {
                        let mut v = serde_json::json!({
                            "step_name": r.step_name,
                            "success": r.success,
                            "completed_at": r.completed_at.to_rfc3339(),
                        });
                        if let Some(ref err) = r.error {
                            v["error"] = serde_json::Value::String(err.clone());
                        }
                        v
                    }
                    None => serde_json::Value::Null,
                })
                .collect();

            let status_str = match chain_state.status {
                ChainStatus::Running => "running",
                ChainStatus::Completed => "completed",
                ChainStatus::Failed => "failed",
                ChainStatus::Cancelled => "cancelled",
                ChainStatus::TimedOut => "timed_out",
            };

            let mut outcome_details = serde_json::json!({
                "chain_name": chain_state.chain_name,
                "total_steps": chain_state.total_steps,
                "completed_steps": chain_state.step_results.iter().filter(|r| r.is_some()).count(),
                "current_step": chain_state.current_step,
                "status": status_str,
                "cancel_reason": chain_state.cancel_reason,
                "cancelled_by": chain_state.cancelled_by,
                "step_results": step_results_json,
            });
            if !chain_state.execution_path.is_empty() {
                outcome_details["execution_path"] = serde_json::json!(chain_state.execution_path);
            }

            #[allow(clippy::cast_possible_wrap)]
            let expires_at = self
                .audit_ttl_seconds
                .map(|secs| chain_state.started_at + chrono::Duration::seconds(secs as i64));

            let action_payload = if self.audit_store_payload {
                Some(chain_state.origin_action.payload.clone())
            } else {
                None
            };

            let duration_ms = (now - chain_state.started_at).num_milliseconds();
            #[allow(clippy::cast_sign_loss)]
            let duration_ms = duration_ms.max(0) as u64;

            let record = AuditRecord {
                id: uuid::Uuid::now_v7().to_string(),
                action_id: chain_state.origin_action.id.to_string(),
                chain_id: Some(chain_state.chain_id.clone()),
                namespace: chain_state.namespace.clone(),
                tenant: chain_state.tenant.clone(),
                provider: "chain".to_owned(),
                action_type: chain_state.chain_name.clone(),
                verdict: "chain".to_owned(),
                matched_rule: None,
                outcome: outcome.to_owned(),
                action_payload,
                verdict_details: serde_json::json!({}),
                outcome_details,
                metadata: enrich_audit_metadata(&chain_state.origin_action),
                dispatched_at: chain_state.started_at,
                completed_at: now,
                duration_ms,
                expires_at,
                caller_id: String::new(),
                auth_method: String::new(),
            };

            let audit = Arc::clone(audit);
            self.audit_tracker.spawn(async move {
                if let Err(e) = audit.record(record).await {
                    warn!(error = %e, "chain terminal audit recording failed");
                }
            });
        }
    }

    /// Get the current state of a chain execution.
    pub async fn get_chain_status(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
    ) -> Result<Option<ChainState>, GatewayError> {
        let chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, chain_id);
        match self.state.get(&chain_key).await? {
            Some(raw) => {
                let json = self.decrypt_state_value(&raw)?;
                let state: ChainState = serde_json::from_str(&json).map_err(|e| {
                    GatewayError::ChainError(format!("failed to deserialize chain state: {e}"))
                })?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// List chain executions, optionally filtered by status.
    pub async fn list_chains(
        &self,
        namespace: &str,
        tenant: &str,
        status_filter: Option<&ChainStatus>,
    ) -> Result<Vec<ChainState>, GatewayError> {
        // Scan the pending chains index for active chains.
        let entries = self
            .state
            .scan_keys(namespace, tenant, KeyKind::PendingChains, None)
            .await?;

        let mut chains = Vec::new();
        for (_, val) in &entries {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(val)
                && let Some(chain_id) = parsed.get("chain_id").and_then(|v| v.as_str())
                && let Ok(Some(state)) = self.get_chain_status(namespace, tenant, chain_id).await
                && (status_filter.is_none() || status_filter == Some(&state.status))
            {
                chains.push(state);
            }
        }

        Ok(chains)
    }

    /// Cancel a running chain. Sets status to `Cancelled` and removes from pending index.
    ///
    /// After updating the chain state, dispatches a cancel notification through
    /// the gateway pipeline. The notification target is taken from the chain
    /// config's `on_cancel` field, falling back to provider `"webhook"` and
    /// action type `"chain_cancelled"`.
    pub async fn cancel_chain(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        reason: Option<String>,
        cancelled_by: Option<String>,
    ) -> Result<ChainState, GatewayError> {
        let chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, chain_id);

        let lock_name = format!("chain:{chain_id}");
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let state_raw = self
            .state
            .get(&chain_key)
            .await?
            .ok_or_else(|| GatewayError::ChainError(format!("chain not found: {chain_id}")))?;
        let state_json = self.decrypt_state_value(&state_raw)?;
        let mut chain_state: ChainState = serde_json::from_str(&state_json).map_err(|e| {
            GatewayError::ChainError(format!("failed to deserialize chain state: {e}"))
        })?;

        if chain_state.status != ChainStatus::Running {
            guard
                .release()
                .await
                .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
            return Err(GatewayError::ChainError(format!(
                "chain is not running (status: {:?})",
                chain_state.status
            )));
        }

        let cancelled_at = Utc::now();
        chain_state.status = ChainStatus::Cancelled;
        chain_state.updated_at = cancelled_at;
        chain_state.cancel_reason.clone_from(&reason);
        chain_state.cancelled_by.clone_from(&cancelled_by);
        self.persist_chain_state(&chain_key, &chain_state, self.completed_chain_ttl)
            .await?;
        self.cleanup_pending_chain(namespace, tenant, chain_id)
            .await?;
        self.metrics.increment_chains_cancelled();
        self.emit_chain_terminal_audit(&chain_state, "chain_cancelled");
        self.emit_stream_event(StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: Utc::now(),
            event_type: StreamEventType::ChainCompleted {
                chain_id: chain_id.to_string(),
                status: "cancelled".to_string(),
                execution_path: chain_state.execution_path.clone(),
            },
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            action_type: Some(chain_state.chain_name.clone()),
            action_id: Some(chain_state.origin_action.id.to_string()),
        });

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        info!(chain_id = %chain_id, "chain cancelled");

        // Dispatch a cancel notification through the gateway pipeline.
        let chain_config = self.chains.get(&chain_state.chain_name);
        let (notify_provider, notify_action_type) = chain_config
            .and_then(|c| c.on_cancel.as_ref())
            .map_or(("webhook", "chain_cancelled"), |t| {
                (t.provider.as_str(), t.action_type.as_str())
            });

        let notification_payload = serde_json::json!({
            "chain_id": chain_id,
            "chain_name": chain_state.chain_name,
            "cancel_reason": reason,
            "cancelled_by": cancelled_by,
            "current_step": chain_state.current_step,
            "total_steps": chain_state.total_steps,
            "cancelled_at": cancelled_at.to_rfc3339(),
        });

        let notification = Action::new(
            namespace,
            tenant,
            notify_provider,
            notify_action_type,
            notification_payload,
        );

        match self.dispatch(notification, None).await {
            Ok(outcome) => {
                debug!(
                    chain_id = %chain_id,
                    ?outcome,
                    "chain cancel notification dispatched"
                );
            }
            Err(e) => {
                warn!(
                    chain_id = %chain_id,
                    error = %e,
                    "failed to dispatch chain cancel notification"
                );
            }
        }

        Ok(chain_state)
    }

    /// Get the full approval record for the given ID.
    pub async fn get_approval_record(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<Option<ApprovalRecord>, GatewayError> {
        let approval_key = StateKey::new(namespace, tenant, KeyKind::Approval, id);
        match self.state.get(&approval_key).await? {
            Some(raw) => {
                let val = self.decrypt_state_value(&raw)?;
                let record: ApprovalRecord = serde_json::from_str(&val).map_err(|e| {
                    GatewayError::Configuration(format!("corrupt approval record: {e}"))
                })?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Execute an approved action by namespace, tenant, ID, and HMAC signature.
    ///
    /// Verifies the HMAC signature, atomically claims the approval, re-evaluates
    /// rules (TOCTOU protection), then executes the original action. If any step
    /// after claiming fails, the claim key is released so the approval can be
    /// retried.
    pub async fn execute_approval(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<ActionOutcome, GatewayError> {
        // 1. Verify HMAC signature (includes expires_at to prevent replay after expiry)
        if !self.verify_approval_sig(namespace, tenant, id, expires_at, sig, kid) {
            return Err(GatewayError::ApprovalNotFound);
        }

        // 2. Atomically claim the approval (first writer wins)
        let claim_key = StateKey::new(namespace, tenant, KeyKind::Approval, format!("{id}:claim"));
        let is_claimed = self
            .state
            .check_and_set(&claim_key, "approved", Some(Duration::from_secs(86400)))
            .await?;
        if !is_claimed {
            return Err(GatewayError::ApprovalAlreadyDecided(
                "concurrent update".into(),
            ));
        }

        // Execute the rest, releasing the claim on failure
        let result = self.execute_approval_inner(namespace, tenant, id).await;
        if result.is_err() {
            let _ = self.state.delete(&claim_key).await;
        }
        result
    }

    /// Inner logic for `execute_approval`, called after the claim is acquired.
    #[allow(clippy::too_many_lines)]
    async fn execute_approval_inner(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<ActionOutcome, GatewayError> {
        // 3. Read the approval record
        let approval_key = StateKey::new(namespace, tenant, KeyKind::Approval, id);
        let raw = self
            .state
            .get(&approval_key)
            .await?
            .ok_or(GatewayError::ApprovalNotFound)?;
        let val = self.decrypt_state_value(&raw)?;
        let record: ApprovalRecord = serde_json::from_str(&val)
            .map_err(|e| GatewayError::Configuration(format!("corrupt approval record: {e}")))?;

        if record.status != "pending" {
            return Err(GatewayError::ApprovalAlreadyDecided(record.status));
        }

        // 4. Update status to "approved"
        let mut updated = record.clone();
        updated.status = "approved".to_string();
        updated.decided_at = Some(Utc::now());
        let updated_json = serde_json::to_string(&updated).map_err(|e| {
            GatewayError::Configuration(format!("failed to serialize approval: {e}"))
        })?;
        let updated_encrypted = self.encrypt_state_value(&updated_json)?;
        self.state
            .set(&approval_key, &updated_encrypted, None)
            .await?;

        self.emit_stream_event(StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: Utc::now(),
            event_type: StreamEventType::ApprovalResolved {
                approval_id: id.to_string(),
                decision: "approved".to_string(),
            },
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            action_type: Some(record.action.action_type.clone()),
            action_id: Some(record.action.id.to_string()),
        });

        // 5. TOCTOU: re-evaluate rules against the stored action
        let action = &record.action;
        let mut eval_ctx = EvalContext::new(action, self.state.as_ref(), &self.environment);
        if let Some(ref emb) = self.embedding {
            eval_ctx = eval_ctx.with_embedding(Arc::clone(emb));
        }
        if let Some(tz) = self.default_timezone {
            eval_ctx = eval_ctx.with_timezone(tz);
        }
        let verdict = self.engine.evaluate(&eval_ctx).await?;

        match &verdict {
            // Rules now suppress/deny => refuse execution
            RuleVerdict::Suppress(rule) | RuleVerdict::Deny(rule) => {
                info!(
                    rule = %rule,
                    "rules changed since approval, action suppressed on re-evaluation"
                );
                Ok(ActionOutcome::Suppressed { rule: rule.clone() })
            }
            // Other verdicts (reroute, throttle, modify) => apply them
            RuleVerdict::Reroute {
                rule: _,
                target_provider,
            } => self.handle_reroute(action, target_provider).await,
            RuleVerdict::Throttle {
                rule: _,
                max_count: _,
                window_seconds,
            } => Ok(ActionOutcome::Throttled {
                retry_after: Duration::from_secs(*window_seconds),
            }),
            RuleVerdict::Modify { rule: _, changes } => {
                let mut modified = action.clone();
                json_patch::merge(&mut modified.payload, changes);
                Ok(self.execute_action(&modified).await)
            }
            // Allow, RequestApproval (human already approved), dedup, state machine, group => execute
            _ => {
                let outcome = self.execute_action(action).await;
                Ok(outcome)
            }
        }
    }

    /// Reject a pending approval by namespace, tenant, ID, and HMAC signature.
    ///
    /// If any step after claiming fails, the claim key is released so the
    /// approval can be retried.
    pub async fn reject_approval(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<(), GatewayError> {
        // 1. Verify HMAC signature (includes expires_at to prevent replay after expiry)
        if !self.verify_approval_sig(namespace, tenant, id, expires_at, sig, kid) {
            return Err(GatewayError::ApprovalNotFound);
        }

        // 2. Atomically claim the approval (first writer wins)
        let claim_key = StateKey::new(namespace, tenant, KeyKind::Approval, format!("{id}:claim"));
        let is_claimed = self
            .state
            .check_and_set(&claim_key, "rejected", Some(Duration::from_secs(86400)))
            .await?;
        if !is_claimed {
            return Err(GatewayError::ApprovalAlreadyDecided(
                "concurrent update".into(),
            ));
        }

        // Execute the rest, releasing the claim on failure
        let result = self.reject_approval_inner(namespace, tenant, id).await;
        if result.is_err() {
            let _ = self.state.delete(&claim_key).await;
        }
        result
    }

    /// Inner logic for `reject_approval`, called after the claim is acquired.
    async fn reject_approval_inner(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<(), GatewayError> {
        // 3. Read the approval record
        let approval_key = StateKey::new(namespace, tenant, KeyKind::Approval, id);
        let raw = self
            .state
            .get(&approval_key)
            .await?
            .ok_or(GatewayError::ApprovalNotFound)?;
        let val = self.decrypt_state_value(&raw)?;
        let record: ApprovalRecord = serde_json::from_str(&val)
            .map_err(|e| GatewayError::Configuration(format!("corrupt approval record: {e}")))?;

        if record.status != "pending" {
            return Err(GatewayError::ApprovalAlreadyDecided(record.status));
        }

        // 4. Update status to "rejected"
        let mut updated = record.clone();
        updated.status = "rejected".to_string();
        updated.decided_at = Some(Utc::now());
        let updated_json = serde_json::to_string(&updated).map_err(|e| {
            GatewayError::Configuration(format!("failed to serialize approval: {e}"))
        })?;
        let updated_encrypted = self.encrypt_state_value(&updated_json)?;
        self.state
            .set(&approval_key, &updated_encrypted, None)
            .await?;

        self.emit_stream_event(StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: Utc::now(),
            event_type: StreamEventType::ApprovalResolved {
                approval_id: id.to_string(),
                decision: "rejected".to_string(),
            },
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            action_type: Some(record.action.action_type.clone()),
            action_id: Some(record.action.id.to_string()),
        });

        Ok(())
    }

    /// Retry sending the notification for a pending approval.
    ///
    /// Re-reads the approval record, re-sends the notification via the provider
    /// specified in the original rule, and updates `notification_sent` on success.
    /// Returns `true` if the notification was successfully sent.
    #[allow(clippy::too_many_lines)]
    pub async fn retry_approval_notification(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<bool, GatewayError> {
        // Read the approval record
        let approval_key = StateKey::new(namespace, tenant, KeyKind::Approval, id);
        let raw = self
            .state
            .get(&approval_key)
            .await?
            .ok_or(GatewayError::ApprovalNotFound)?;
        let val = self.decrypt_state_value(&raw)?;
        let record: ApprovalRecord = serde_json::from_str(&val)
            .map_err(|e| GatewayError::Configuration(format!("corrupt approval record: {e}")))?;

        if record.status != "pending" || record.notification_sent {
            return Ok(false);
        }

        // Check if expired
        if record.expires_at <= Utc::now() {
            return Ok(false);
        }

        // Compute HMAC signature for the URLs (includes expires_at)
        let expires_ts = record.expires_at.timestamp();
        let (sig, kid) = self.compute_approval_sig(namespace, tenant, id, expires_ts);

        let external_url = self
            .external_url
            .as_deref()
            .unwrap_or("http://localhost:8080");
        let approve_url = format!(
            "{external_url}/v1/approvals/{namespace}/{tenant}/{id}/approve?sig={sig}&expires_at={expires_ts}&kid={kid}"
        );
        let reject_url = format!(
            "{external_url}/v1/approvals/{namespace}/{tenant}/{id}/reject?sig={sig}&expires_at={expires_ts}&kid={kid}"
        );

        let notification_payload = serde_json::json!({
            "subject": format!("Approval Required: {}", record.action.action_type),
            "body": format!(
                "Action '{}' requires approval.\n\nReason: {}\n\nApprove: {}\nReject: {}",
                record.action.action_type,
                record.message.as_deref().unwrap_or("Approval required"),
                approve_url,
                reject_url,
            ),
            "approval_url": &approve_url,
            "reject_url": &reject_url,
            "action_id": record.action.id.to_string(),
            "action_type": &record.action.action_type,
            "namespace": namespace,
            "tenant": tenant,
            "expires_at": record.expires_at.to_rfc3339(),
        });

        // Look up the notification provider from the rule that created this approval.
        // We re-evaluate rules to find the matching RequestApproval rule.
        let mut eval_ctx = EvalContext::new(&record.action, self.state.as_ref(), &self.environment);
        if let Some(ref emb) = self.embedding {
            eval_ctx = eval_ctx.with_embedding(Arc::clone(emb));
        }
        if let Some(tz) = self.default_timezone {
            eval_ctx = eval_ctx.with_timezone(tz);
        }
        let verdict = self.engine.evaluate(&eval_ctx).await?;

        let notify_provider = if let RuleVerdict::RequestApproval {
            notify_provider, ..
        } = &verdict
        {
            notify_provider.clone()
        } else {
            // Rules changed; can't determine the provider
            warn!(
                approval_id = %id,
                "rules changed since approval was created, cannot determine notification provider"
            );
            return Ok(false);
        };

        let Some(provider) = self.providers.get(&notify_provider) else {
            error!(
                provider = %notify_provider,
                approval_id = %id,
                "notification provider not found during retry"
            );
            return Ok(false);
        };

        let notification = Action::new(
            namespace,
            tenant,
            notify_provider.as_str(),
            "approval_notification",
            notification_payload,
        );
        let result = self
            .executor
            .execute(&notification, provider.as_ref())
            .await;

        if let ActionOutcome::Failed(err) = &result {
            error!(
                error = %err.message,
                approval_id = %id,
                "approval notification retry failed"
            );
            return Ok(false);
        }

        // Update the record with notification_sent = true
        let mut updated = record;
        updated.notification_sent = true;
        let updated_json = serde_json::to_string(&updated).map_err(|e| {
            GatewayError::Configuration(format!("failed to serialize approval: {e}"))
        })?;
        let updated_encrypted = self.encrypt_state_value(&updated_json)?;

        // Preserve the original TTL by computing remaining time
        let remaining = updated.expires_at - Utc::now();
        #[allow(clippy::cast_sign_loss)]
        let ttl = if remaining.num_seconds() > 0 {
            Some(Duration::from_secs(remaining.num_seconds() as u64))
        } else {
            None
        };
        self.state
            .set(&approval_key, &updated_encrypted, ttl)
            .await?;

        info!(approval_id = %id, "approval notification retry succeeded");
        Ok(true)
    }

    /// Get the status of an approval by namespace, tenant, ID, and HMAC signature.
    pub async fn get_approval_status(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<Option<ApprovalStatus>, GatewayError> {
        // Verify HMAC signature
        if !self.verify_approval_sig(namespace, tenant, id, expires_at, sig, kid) {
            return Ok(None);
        }

        let approval_key = StateKey::new(namespace, tenant, KeyKind::Approval, id);
        let Some(raw) = self.state.get(&approval_key).await? else {
            return Ok(None);
        };

        let val = self.decrypt_state_value(&raw)?;
        let record: ApprovalRecord = serde_json::from_str(&val)
            .map_err(|e| GatewayError::Configuration(format!("corrupt approval record: {e}")))?;

        Ok(Some(ApprovalStatus {
            token: record.token,
            status: record.status,
            rule: record.rule,
            created_at: record.created_at,
            expires_at: record.expires_at,
            decided_at: record.decided_at,
            message: record.message,
        }))
    }

    /// List pending approvals for a namespace/tenant.
    pub async fn list_pending_approvals(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<Vec<ApprovalStatus>, GatewayError> {
        let entries = self
            .state
            .scan_keys(namespace, tenant, KeyKind::Approval, None)
            .await?;

        let mut results = Vec::new();
        for (_key, raw) in entries {
            let Ok(val) = self.decrypt_state_value(&raw) else {
                continue;
            };
            if let Ok(record) = serde_json::from_str::<ApprovalRecord>(&val) {
                results.push(ApprovalStatus {
                    token: record.token,
                    status: record.status,
                    rule: record.rule,
                    created_at: record.created_at,
                    expires_at: record.expires_at,
                    decided_at: record.decided_at,
                    message: record.message,
                });
            }
        }
        Ok(results)
    }

    /// Register a state machine configuration.
    pub fn register_state_machine(&mut self, config: StateMachineConfig) {
        self.state_machines.insert(config.name.clone(), config);
    }

    /// Get the shared group manager.
    ///
    /// Returns a clone of the `Arc` to allow sharing with a
    /// [`BackgroundProcessor`](crate::background::BackgroundProcessor).
    pub fn group_manager(&self) -> Arc<GroupManager> {
        Arc::clone(&self.group_manager)
    }

    /// Get a reference to the state store.
    pub fn state_store(&self) -> &Arc<dyn StateStore> {
        &self.state
    }

    /// Get a clone of the broadcast sender for SSE event streaming.
    ///
    /// Callers can use `subscribe()` on the returned sender to create
    /// new receivers, or hold the sender to emit additional events.
    pub fn stream_tx(&self) -> &tokio::sync::broadcast::Sender<StreamEvent> {
        &self.stream_tx
    }

    /// Emit a stream event on the broadcast channel (fire-and-forget).
    ///
    /// No-op if there are no subscribers. Does not propagate send errors.
    fn emit_stream_event(&self, event: StreamEvent) {
        let _ = self.stream_tx.send(event);
    }
}

/// A read-only wrapper for a [`StateStore`] that allows overriding specific keys.
/// Used by the Rule Playground to simulate different state conditions.
struct PlaygroundStateStore<'a> {
    inner: &'a dyn acteon_state::StateStore,
    overrides: HashMap<String, String>,
}

#[async_trait::async_trait]
impl acteon_state::StateStore for PlaygroundStateStore<'_> {
    async fn get(
        &self,
        key: &acteon_state::StateKey,
    ) -> Result<Option<String>, acteon_state::StateError> {
        // Try the overrides map first.  We check for both the full canonical key
        // and just the ID part for convenience.
        if let Some(val) = self.overrides.get(&key.canonical()) {
            return Ok(Some(val.clone()));
        }
        if let Some(val) = self.overrides.get(&key.id) {
            return Ok(Some(val.clone()));
        }
        self.inner.get(key).await
    }

    // All other methods are no-ops or errors since the playground is read-only.
    async fn set(
        &self,
        _: &acteon_state::StateKey,
        _: &str,
        _: Option<Duration>,
    ) -> Result<(), acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn check_and_set(
        &self,
        _: &acteon_state::StateKey,
        _: &str,
        _: Option<Duration>,
    ) -> Result<bool, acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn delete(&self, _: &acteon_state::StateKey) -> Result<bool, acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn increment(
        &self,
        _: &acteon_state::StateKey,
        _: i64,
        _: Option<Duration>,
    ) -> Result<i64, acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn compare_and_swap(
        &self,
        _: &acteon_state::StateKey,
        _: u64,
        _: &str,
        _: Option<Duration>,
    ) -> Result<acteon_state::CasResult, acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn scan_keys(
        &self,
        ns: &str,
        t: &str,
        k: acteon_state::KeyKind,
        p: Option<&str>,
    ) -> Result<Vec<(String, String)>, acteon_state::StateError> {
        self.inner.scan_keys(ns, t, k, p).await
    }
    async fn scan_keys_by_kind(
        &self,
        k: acteon_state::KeyKind,
    ) -> Result<Vec<(String, String)>, acteon_state::StateError> {
        self.inner.scan_keys_by_kind(k).await
    }
    async fn index_timeout(
        &self,
        _: &acteon_state::StateKey,
        _: i64,
    ) -> Result<(), acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn remove_timeout_index(
        &self,
        _: &acteon_state::StateKey,
    ) -> Result<(), acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn get_expired_timeouts(
        &self,
        now: i64,
    ) -> Result<Vec<String>, acteon_state::StateError> {
        self.inner.get_expired_timeouts(now).await
    }
    async fn index_chain_ready(
        &self,
        _: &acteon_state::StateKey,
        _: i64,
    ) -> Result<(), acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn remove_chain_ready_index(
        &self,
        _: &acteon_state::StateKey,
    ) -> Result<(), acteon_state::StateError> {
        Err(acteon_state::StateError::Backend(
            "Playground state store is read-only".into(),
        ))
    }
    async fn get_ready_chains(&self, now: i64) -> Result<Vec<String>, acteon_state::StateError> {
        self.inner.get_ready_chains(now).await
    }
}

/// Helper to wrap a reference to a [`StateStore`] as a trait object.
struct BorrowedStateStore<'a>(&'a dyn acteon_state::StateStore);

#[async_trait::async_trait]
impl acteon_state::StateStore for BorrowedStateStore<'_> {
    async fn get(
        &self,
        k: &acteon_state::StateKey,
    ) -> Result<Option<String>, acteon_state::StateError> {
        self.0.get(k).await
    }
    async fn set(
        &self,
        k: &acteon_state::StateKey,
        v: &str,
        d: Option<Duration>,
    ) -> Result<(), acteon_state::StateError> {
        self.0.set(k, v, d).await
    }
    async fn check_and_set(
        &self,
        k: &acteon_state::StateKey,
        v: &str,
        d: Option<Duration>,
    ) -> Result<bool, acteon_state::StateError> {
        self.0.check_and_set(k, v, d).await
    }
    async fn delete(&self, k: &acteon_state::StateKey) -> Result<bool, acteon_state::StateError> {
        self.0.delete(k).await
    }
    async fn increment(
        &self,
        k: &acteon_state::StateKey,
        d: i64,
        t: Option<Duration>,
    ) -> Result<i64, acteon_state::StateError> {
        self.0.increment(k, d, t).await
    }
    async fn compare_and_swap(
        &self,
        k: &acteon_state::StateKey,
        ev: u64,
        nv: &str,
        t: Option<Duration>,
    ) -> Result<acteon_state::CasResult, acteon_state::StateError> {
        self.0.compare_and_swap(k, ev, nv, t).await
    }
    async fn scan_keys(
        &self,
        ns: &str,
        t: &str,
        k: acteon_state::KeyKind,
        p: Option<&str>,
    ) -> Result<Vec<(String, String)>, acteon_state::StateError> {
        self.0.scan_keys(ns, t, k, p).await
    }
    async fn scan_keys_by_kind(
        &self,
        k: acteon_state::KeyKind,
    ) -> Result<Vec<(String, String)>, acteon_state::StateError> {
        self.0.scan_keys_by_kind(k).await
    }
    async fn index_timeout(
        &self,
        k: &acteon_state::StateKey,
        e: i64,
    ) -> Result<(), acteon_state::StateError> {
        self.0.index_timeout(k, e).await
    }
    async fn remove_timeout_index(
        &self,
        k: &acteon_state::StateKey,
    ) -> Result<(), acteon_state::StateError> {
        self.0.remove_timeout_index(k).await
    }
    async fn get_expired_timeouts(&self, n: i64) -> Result<Vec<String>, acteon_state::StateError> {
        self.0.get_expired_timeouts(n).await
    }
    async fn index_chain_ready(
        &self,
        k: &acteon_state::StateKey,
        r: i64,
    ) -> Result<(), acteon_state::StateError> {
        self.0.index_chain_ready(k, r).await
    }
    async fn remove_chain_ready_index(
        &self,
        k: &acteon_state::StateKey,
    ) -> Result<(), acteon_state::StateError> {
        self.0.remove_chain_ready_index(k).await
    }
    async fn get_ready_chains(&self, n: i64) -> Result<Vec<String>, acteon_state::StateError> {
        self.0.get_ready_chains(n).await
    }
}

// -- Audit helpers -----------------------------------------------------------

/// Extract the matched rule name from a `RuleVerdict`, if any.
fn matched_rule_name(verdict: &RuleVerdict) -> Option<String> {
    match verdict {
        RuleVerdict::Allow(_) | RuleVerdict::Deduplicate { .. } => None,
        RuleVerdict::Deny(rule)
        | RuleVerdict::Suppress(rule)
        | RuleVerdict::Reroute { rule, .. }
        | RuleVerdict::Throttle { rule, .. }
        | RuleVerdict::Modify { rule, .. }
        | RuleVerdict::StateMachine { rule, .. }
        | RuleVerdict::Group { rule, .. }
        | RuleVerdict::RequestApproval { rule, .. }
        | RuleVerdict::Chain { rule, .. }
        | RuleVerdict::Schedule { rule, .. } => Some(rule.clone()),
    }
}

/// Extract a string tag from an `ActionOutcome`.
fn outcome_tag(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "executed",
        ActionOutcome::Deduplicated => "deduplicated",
        ActionOutcome::Suppressed { .. } => "suppressed",
        ActionOutcome::Rerouted { .. } => "rerouted",
        ActionOutcome::Throttled { .. } => "throttled",
        ActionOutcome::Failed(_) => "failed",
        ActionOutcome::Grouped { .. } => "grouped",
        ActionOutcome::StateChanged { .. } => "state_changed",
        ActionOutcome::PendingApproval { .. } => "pending_approval",
        ActionOutcome::ChainStarted { .. } => "chain_started",
        ActionOutcome::DryRun { .. } => "dry_run",
        ActionOutcome::CircuitOpen { .. } => "circuit_open",
        ActionOutcome::Scheduled { .. } => "scheduled",
        ActionOutcome::RecurringCreated { .. } => "recurring_created",
        ActionOutcome::QuotaExceeded { .. } => "quota_exceeded",
    }
}

/// Enrich serialized action metadata with extra `Action` fields so that
/// replays can reconstruct the full action. System fields use a `__` prefix
/// to distinguish them from user-supplied labels.
fn enrich_audit_metadata(action: &Action) -> serde_json::Value {
    let mut meta = serde_json::to_value(&action.metadata).unwrap_or_default();
    if let Some(obj) = meta.as_object_mut() {
        if let Some(k) = &action.dedup_key {
            obj.insert("__dedup_key".into(), serde_json::json!(k));
        }
        if let Some(f) = &action.fingerprint {
            obj.insert("__fingerprint".into(), serde_json::json!(f));
        }
        if let Some(s) = &action.status {
            obj.insert("__status".into(), serde_json::json!(s));
        }
        if let Some(t) = action.starts_at {
            obj.insert("__starts_at".into(), serde_json::json!(t));
        }
        if let Some(t) = action.ends_at {
            obj.insert("__ends_at".into(), serde_json::json!(t));
        }
    }
    meta
}

/// Build an `AuditRecord` from the dispatch context.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_audit_record(
    id: String,
    action: &Action,
    verdict: &RuleVerdict,
    outcome: &ActionOutcome,
    dispatched_at: chrono::DateTime<chrono::Utc>,
    elapsed: Duration,
    ttl_seconds: Option<u64>,
    store_payload: bool,
    caller: Option<&Caller>,
) -> AuditRecord {
    let completed_at = Utc::now();
    #[allow(clippy::cast_possible_wrap)]
    let expires_at = ttl_seconds.map(|secs| dispatched_at + chrono::Duration::seconds(secs as i64));

    let action_payload = if store_payload {
        Some(action.payload.clone())
    } else {
        None
    };

    let outcome_details = match outcome {
        ActionOutcome::Executed(resp) => serde_json::json!({
            "status": format!("{:?}", resp.status),
        }),
        ActionOutcome::Failed(err) => serde_json::json!({
            "code": err.code,
            "message": err.message,
            "retryable": err.retryable,
            "attempts": err.attempts,
        }),
        ActionOutcome::Suppressed { rule } => serde_json::json!({ "rule": rule }),
        ActionOutcome::Rerouted {
            original_provider,
            new_provider,
            ..
        } => serde_json::json!({
            "original_provider": original_provider,
            "new_provider": new_provider,
        }),
        ActionOutcome::Throttled { retry_after } => {
            serde_json::json!({ "retry_after_secs": retry_after.as_secs() })
        }
        ActionOutcome::Deduplicated => serde_json::json!({}),
        ActionOutcome::Grouped {
            group_id,
            group_size,
            notify_at,
        } => serde_json::json!({
            "group_id": group_id,
            "group_size": group_size,
            "notify_at": notify_at.to_rfc3339(),
        }),
        ActionOutcome::StateChanged {
            fingerprint,
            previous_state,
            new_state,
            notify,
        } => serde_json::json!({
            "fingerprint": fingerprint,
            "previous_state": previous_state,
            "new_state": new_state,
            "notify": notify,
        }),
        ActionOutcome::PendingApproval {
            approval_id,
            expires_at,
            notification_sent,
            ..
        } => serde_json::json!({
            "approval_id": approval_id,
            "expires_at": expires_at.to_rfc3339(),
            "notification_sent": notification_sent,
        }),
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            total_steps,
            first_step,
        } => serde_json::json!({
            "chain_id": chain_id,
            "chain_name": chain_name,
            "total_steps": total_steps,
            "first_step": first_step,
        }),
        ActionOutcome::DryRun {
            verdict,
            matched_rule,
            would_be_provider,
        } => serde_json::json!({
            "verdict": verdict,
            "matched_rule": matched_rule,
            "would_be_provider": would_be_provider,
        }),
        ActionOutcome::CircuitOpen {
            provider,
            fallback_chain,
        } => serde_json::json!({
            "provider": provider,
            "fallback_chain": fallback_chain,
        }),
        ActionOutcome::Scheduled {
            action_id,
            scheduled_for,
        } => serde_json::json!({
            "action_id": action_id,
            "scheduled_for": scheduled_for.to_rfc3339(),
        }),
        ActionOutcome::RecurringCreated {
            recurring_id,
            cron_expr,
            next_execution_at,
        } => serde_json::json!({
            "recurring_id": recurring_id,
            "cron_expr": cron_expr,
            "next_execution_at": next_execution_at.map(|t| t.to_rfc3339()),
        }),
        ActionOutcome::QuotaExceeded {
            tenant,
            limit,
            used,
            overage_behavior,
        } => serde_json::json!({
            "tenant": tenant,
            "limit": limit,
            "used": used,
            "overage_behavior": overage_behavior,
        }),
    };

    let chain_id = if let ActionOutcome::ChainStarted { chain_id, .. } = outcome {
        Some(chain_id.clone())
    } else {
        None
    };

    AuditRecord {
        id,
        action_id: action.id.to_string(),
        chain_id,
        namespace: action.namespace.to_string(),
        tenant: action.tenant.to_string(),
        provider: action.provider.to_string(),
        action_type: action.action_type.clone(),
        verdict: verdict.as_tag().to_owned(),
        matched_rule: matched_rule_name(verdict),
        outcome: outcome_tag(outcome).to_owned(),
        action_payload,
        verdict_details: serde_json::json!({ "verdict": verdict.as_tag() }),
        outcome_details,
        metadata: enrich_audit_metadata(action),
        dispatched_at,
        completed_at,
        duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        expires_at,
        caller_id: caller.map_or_else(String::new, |c| c.id.clone()),
        auth_method: caller.map_or_else(String::new, |c| c.auth_method.clone()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
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
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
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
        let outcome1 = gw.dispatch(action.clone(), None).await.unwrap();
        assert!(matches!(outcome1, ActionOutcome::Executed(_)));

        // Second dispatch with same dedup key should be deduplicated.
        let outcome2 = gw.dispatch(action, None).await.unwrap();
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

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.suppressed, 1);
    }

    #[tokio::test]
    async fn dispatch_deny_maps_to_suppressed() {
        let rules = vec![Rule::new("deny-all", Expr::Bool(true), RuleAction::Deny)];
        let gw = build_gateway(rules);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
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

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
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

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
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

        let outcome = gw.dispatch(action, None).await.unwrap();
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

        let result = gw.dispatch(test_action(), None).await;
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

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
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

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
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
        let results = gw.dispatch_batch(actions, None).await;

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
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Executed(_)));

        // Reload with a suppress rule.
        gw.reload_rules(vec![Rule::new(
            "block",
            Expr::Bool(true),
            RuleAction::Suppress,
        )]);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));
    }

    #[tokio::test]
    async fn metrics_increment_correctly() {
        let gw = build_gateway(vec![]);

        // Dispatch several actions.
        for _ in 0..5 {
            let _ = gw.dispatch(test_action(), None).await;
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

    // -- Approval test helpers ------------------------------------------------

    use acteon_rules::ir::expr::BinaryOp;
    use acteon_state::{DistributedLock, StateStore};

    struct FailingMockProvider {
        provider_name: String,
    }

    impl FailingMockProvider {
        fn new(name: &str) -> Self {
            Self {
                provider_name: name.to_owned(),
            }
        }
    }

    #[async_trait]
    impl DynProvider for FailingMockProvider {
        fn name(&self) -> &str {
            &self.provider_name
        }

        async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
            // Use Connection (retryable) so circuit breaker counts this as
            // a provider health issue.
            Err(ProviderError::Connection("provider down".into()))
        }

        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    /// Build a condition expression matching `action.action_type == "process_refund"`.
    fn refund_condition() -> Expr {
        Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "action_type".into(),
            )),
            Box::new(Expr::String("process_refund".into())),
        )
    }

    fn approval_rule(timeout_seconds: u64) -> Rule {
        Rule::new(
            "approve-refunds",
            refund_condition(),
            RuleAction::RequestApproval {
                notify_provider: "slack".into(),
                timeout_seconds,
                message: Some("Requires approval".into()),
            },
        )
    }

    fn build_approval_gateway(
        rules: Vec<Rule>,
        providers: Vec<Arc<dyn DynProvider>>,
    ) -> crate::gateway::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        build_approval_gateway_with_state(rules, providers, store, lock)
    }

    fn build_approval_gateway_with_state(
        rules: Vec<Rule>,
        providers: Vec<Arc<dyn DynProvider>>,
        store: Arc<dyn StateStore>,
        lock: Arc<dyn DistributedLock>,
    ) -> crate::gateway::Gateway {
        let mut builder = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(rules)
            .approval_secret(b"test-secret-key-for-approvals!!")
            .external_url("https://test.example.com");
        for p in providers {
            builder = builder.provider(p);
        }
        builder.build().expect("gateway should build")
    }

    fn refund_action() -> Action {
        Action::new(
            "payments",
            "tenant-1",
            "payments",
            "process_refund",
            serde_json::json!({"order_id": "ORD-123", "amount": 99.99}),
        )
    }

    fn parse_query_param(url: &str, param: &str) -> Option<String> {
        let query = url.split('?').nth(1)?;
        for pair in query.split('&') {
            let mut kv = pair.splitn(2, '=');
            if kv.next()? == param {
                return kv.next().map(String::from);
            }
        }
        None
    }

    // -- Approval tests -------------------------------------------------------

    #[tokio::test]
    async fn approval_dispatch_returns_pending_with_signed_urls() {
        let gw = build_approval_gateway(
            vec![approval_rule(3600)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(MockProvider::new("slack")),
            ],
        );

        let outcome = gw.dispatch(refund_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                reject_url,
                notification_sent,
                ..
            } => {
                assert!(!approval_id.is_empty(), "approval_id must be non-empty");
                assert!(
                    approve_url.starts_with("https://test.example.com/v1/approvals/"),
                    "approve_url should start with external_url prefix"
                );
                assert!(
                    reject_url.starts_with("https://test.example.com/v1/approvals/"),
                    "reject_url should start with external_url prefix"
                );
                assert!(
                    parse_query_param(&approve_url, "sig").is_some(),
                    "approve_url must contain sig param"
                );
                assert!(
                    parse_query_param(&approve_url, "expires_at").is_some(),
                    "approve_url must contain expires_at param"
                );
                assert!(
                    parse_query_param(&reject_url, "sig").is_some(),
                    "reject_url must contain sig param"
                );
                assert!(
                    parse_query_param(&reject_url, "expires_at").is_some(),
                    "reject_url must contain expires_at param"
                );
                assert!(
                    parse_query_param(&approve_url, "kid").is_some(),
                    "approve_url must contain kid param"
                );
                assert!(
                    parse_query_param(&reject_url, "kid").is_some(),
                    "reject_url must contain kid param"
                );
                assert!(
                    notification_sent,
                    "notification should be sent successfully"
                );
            }
            other => panic!("expected PendingApproval, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn approval_approve_executes_action() {
        let gw = build_approval_gateway(
            vec![approval_rule(3600)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(MockProvider::new("slack")),
            ],
        );

        let outcome = gw.dispatch(refund_action(), None).await.unwrap();
        let (approval_id, approve_url) = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                ..
            } => (approval_id, approve_url),
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        let sig = parse_query_param(&approve_url, "sig").expect("sig param");
        let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
            .expect("expires_at param")
            .parse()
            .expect("expires_at should be an integer");
        let kid = parse_query_param(&approve_url, "kid");

        let result = gw
            .execute_approval(
                "payments",
                "tenant-1",
                &approval_id,
                &sig,
                expires_at,
                kid.as_deref(),
            )
            .await
            .unwrap();
        assert!(
            matches!(result, ActionOutcome::Executed(_)),
            "approved action should execute, got {result:?}"
        );

        let approvals = gw
            .list_pending_approvals("payments", "tenant-1")
            .await
            .unwrap();
        let record = approvals
            .iter()
            .find(|a| a.token == approval_id)
            .expect("approval record should exist");
        assert_eq!(record.status, "approved");
    }

    #[tokio::test]
    async fn approval_reject_updates_status() {
        let gw = build_approval_gateway(
            vec![approval_rule(3600)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(MockProvider::new("slack")),
            ],
        );

        let outcome = gw.dispatch(refund_action(), None).await.unwrap();
        let (approval_id, reject_url) = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                reject_url,
                ..
            } => (approval_id, reject_url),
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        let sig = parse_query_param(&reject_url, "sig").expect("sig param");
        let expires_at: i64 = parse_query_param(&reject_url, "expires_at")
            .expect("expires_at param")
            .parse()
            .expect("expires_at should be an integer");
        let kid = parse_query_param(&reject_url, "kid");

        gw.reject_approval(
            "payments",
            "tenant-1",
            &approval_id,
            &sig,
            expires_at,
            kid.as_deref(),
        )
        .await
        .unwrap();

        let approvals = gw
            .list_pending_approvals("payments", "tenant-1")
            .await
            .unwrap();
        let record = approvals
            .iter()
            .find(|a| a.token == approval_id)
            .expect("approval record should exist");
        assert_eq!(record.status, "rejected");
    }

    #[tokio::test]
    async fn approval_notification_failure_retry_succeeds() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());

        // Gateway A: FailingMockProvider("slack") — notification will fail.
        let gw_a = build_approval_gateway_with_state(
            vec![approval_rule(3600)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(FailingMockProvider::new("slack")),
            ],
            Arc::clone(&store),
            Arc::clone(&lock),
        );

        let outcome = gw_a.dispatch(refund_action(), None).await.unwrap();
        let approval_id = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                notification_sent,
                ..
            } => {
                assert!(
                    !notification_sent,
                    "notification should fail with FailingMockProvider"
                );
                approval_id
            }
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        // Gateway B: shares same state, has a working MockProvider("slack").
        let gw_b = build_approval_gateway_with_state(
            vec![approval_rule(3600)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(MockProvider::new("slack")),
            ],
            Arc::clone(&store),
            Arc::clone(&lock),
        );

        let retried = gw_b
            .retry_approval_notification("payments", "tenant-1", &approval_id)
            .await
            .unwrap();
        assert!(retried, "retry should succeed with working provider");

        // Calling retry again should return false (already sent).
        let retried_again = gw_b
            .retry_approval_notification("payments", "tenant-1", &approval_id)
            .await
            .unwrap();
        assert!(
            !retried_again,
            "second retry should return false (notification already sent)"
        );
    }

    #[tokio::test]
    async fn approval_expired_link_returns_error() {
        let gw = build_approval_gateway(
            vec![approval_rule(2)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(MockProvider::new("slack")),
            ],
        );

        let outcome = gw.dispatch(refund_action(), None).await.unwrap();
        let (approval_id, approve_url) = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                ..
            } => (approval_id, approve_url),
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        let sig = parse_query_param(&approve_url, "sig").expect("sig param");
        let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
            .expect("expires_at param")
            .parse()
            .expect("expires_at should be an integer");
        let kid = parse_query_param(&approve_url, "kid");

        // Wait for the approval to expire (2-second timeout + buffer).
        tokio::time::sleep(Duration::from_secs(3)).await;

        let result = gw
            .execute_approval(
                "payments",
                "tenant-1",
                &approval_id,
                &sig,
                expires_at,
                kid.as_deref(),
            )
            .await;
        assert!(
            result.is_err(),
            "expired approval should return an error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn approval_key_rotation_old_key_still_verifies() {
        use crate::gateway::{ApprovalKey, ApprovalKeySet};

        let old_secret = b"old-secret-key-for-rotation!!!!".to_vec();
        let new_secret = b"new-secret-key-for-rotation!!!!".to_vec();

        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        // Build a gateway with only the old key (simulating the previous deployment).
        let old_keys = ApprovalKeySet::new(vec![ApprovalKey {
            kid: "k1".into(),
            secret: old_secret.clone(),
        }]);

        let gw_old = GatewayBuilder::new()
            .state(Arc::clone(&store) as Arc<dyn StateStore>)
            .lock(Arc::clone(&lock) as Arc<dyn DistributedLock>)
            .rules(vec![approval_rule(3600)])
            .provider(Arc::new(MockProvider::new("payments")))
            .provider(Arc::new(MockProvider::new("slack")))
            .approval_keys(old_keys)
            .external_url("https://test.example.com")
            .build()
            .expect("gateway should build");

        // Dispatch to get a signed URL from the old key.
        let outcome = gw_old.dispatch(refund_action(), None).await.unwrap();
        let (approval_id, approve_url) = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                ..
            } => (approval_id, approve_url),
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        let sig = parse_query_param(&approve_url, "sig").expect("sig param");
        let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
            .expect("expires_at param")
            .parse()
            .expect("expires_at should be an integer");
        let kid = parse_query_param(&approve_url, "kid");
        assert_eq!(kid.as_deref(), Some("k1"), "URL should contain kid=k1");

        // Build a new gateway with rotated keys: k2 is current, k1 is still accepted.
        let rotated_keys = ApprovalKeySet::new(vec![
            ApprovalKey {
                kid: "k2".into(),
                secret: new_secret,
            },
            ApprovalKey {
                kid: "k1".into(),
                secret: old_secret,
            },
        ]);

        let gw_new = GatewayBuilder::new()
            .state(store as Arc<dyn StateStore>)
            .lock(lock as Arc<dyn DistributedLock>)
            .rules(vec![approval_rule(3600)])
            .provider(Arc::new(MockProvider::new("payments")))
            .provider(Arc::new(MockProvider::new("slack")))
            .approval_keys(rotated_keys)
            .external_url("https://test.example.com")
            .build()
            .expect("gateway should build");

        // The old-key-signed URL should still verify on the new gateway.
        let result = gw_new
            .execute_approval(
                "payments",
                "tenant-1",
                &approval_id,
                &sig,
                expires_at,
                kid.as_deref(),
            )
            .await
            .unwrap();
        assert!(
            matches!(result, ActionOutcome::Executed(_)),
            "old-key-signed approval should still execute after rotation, got {result:?}"
        );
    }

    #[tokio::test]
    async fn approval_backward_compat_no_kid_in_url() {
        use crate::gateway::ApprovalKeySet;

        let secret = b"compat-test-secret-key-value!!!".to_vec();
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let keys = ApprovalKeySet::from_single(secret);

        let gw = GatewayBuilder::new()
            .state(Arc::clone(&store) as Arc<dyn StateStore>)
            .lock(Arc::clone(&lock) as Arc<dyn DistributedLock>)
            .rules(vec![approval_rule(3600)])
            .provider(Arc::new(MockProvider::new("payments")))
            .provider(Arc::new(MockProvider::new("slack")))
            .approval_keys(keys)
            .external_url("https://test.example.com")
            .build()
            .expect("gateway should build");

        let outcome = gw.dispatch(refund_action(), None).await.unwrap();
        let (approval_id, approve_url) = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                ..
            } => (approval_id, approve_url),
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        let sig = parse_query_param(&approve_url, "sig").expect("sig param");
        let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
            .expect("expires_at param")
            .parse()
            .expect("expires_at should be an integer");

        // Pass kid as None to simulate a legacy URL without kid parameter.
        let result = gw
            .execute_approval("payments", "tenant-1", &approval_id, &sig, expires_at, None)
            .await
            .unwrap();
        assert!(
            matches!(result, ActionOutcome::Executed(_)),
            "verification with kid=None should succeed by trying all keys, got {result:?}"
        );
    }

    #[tokio::test]
    async fn approval_unknown_kid_rejected() {
        let gw = build_approval_gateway(
            vec![approval_rule(3600)],
            vec![
                Arc::new(MockProvider::new("payments")),
                Arc::new(MockProvider::new("slack")),
            ],
        );

        let outcome = gw.dispatch(refund_action(), None).await.unwrap();
        let (approval_id, approve_url) = match outcome {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                ..
            } => (approval_id, approve_url),
            other => panic!("expected PendingApproval, got {other:?}"),
        };

        let sig = parse_query_param(&approve_url, "sig").expect("sig param");
        let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
            .expect("expires_at param")
            .parse()
            .expect("expires_at should be an integer");

        // Pass an unknown kid -- verification should fail.
        let result = gw
            .execute_approval(
                "payments",
                "tenant-1",
                &approval_id,
                &sig,
                expires_at,
                Some("bad"),
            )
            .await;
        assert!(
            result.is_err(),
            "unknown kid should cause verification failure, got {result:?}"
        );
    }

    #[test]
    fn approval_keyset_from_single() {
        use crate::gateway::ApprovalKeySet;

        let secret = b"my-secret".to_vec();
        let ks = ApprovalKeySet::from_single(secret.clone());

        assert_eq!(ks.current().kid, "k0");
        assert_eq!(ks.current().secret, secret);
        assert_eq!(ks.all().len(), 1);
        assert!(ks.get("k0").is_some());
        assert!(ks.get("k1").is_none());
    }

    // -- LLM guardrail tests -------------------------------------------------

    fn build_gateway_with_llm(
        rules: Vec<Rule>,
        evaluator: Arc<dyn acteon_llm::LlmEvaluator>,
        fail_open: bool,
    ) -> crate::gateway::Gateway {
        build_gateway_with_llm_policies(rules, evaluator, fail_open, HashMap::new())
    }

    fn build_gateway_with_llm_policies(
        rules: Vec<Rule>,
        evaluator: Arc<dyn acteon_llm::LlmEvaluator>,
        fail_open: bool,
        llm_policies: HashMap<String, String>,
    ) -> crate::gateway::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(rules)
            .provider(Arc::new(MockProvider::new("email")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .llm_evaluator(evaluator)
            .llm_policy("test policy".to_string())
            .llm_policies(llm_policies)
            .llm_fail_open(fail_open)
            .build()
            .expect("gateway should build")
    }

    #[tokio::test]
    async fn llm_guardrail_blocks_action() {
        let evaluator = Arc::new(acteon_llm::MockLlmEvaluator::denying("unsafe action"));
        let gw = build_gateway_with_llm(vec![], evaluator, true);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Suppressed { ref rule } if rule.contains("LLM guardrail")),
            "LLM deny should produce Suppressed outcome, got {outcome:?}"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.llm_guardrail_denied, 1);
        assert_eq!(snap.llm_guardrail_allowed, 0);
    }

    #[tokio::test]
    async fn llm_guardrail_allows_action() {
        let evaluator = Arc::new(acteon_llm::MockLlmEvaluator::allowing());
        let gw = build_gateway_with_llm(vec![], evaluator, true);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "LLM allow should let action execute, got {outcome:?}"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.llm_guardrail_allowed, 1);
        assert_eq!(snap.llm_guardrail_denied, 0);
    }

    #[tokio::test]
    async fn llm_guardrail_skips_already_denied() {
        // Use a FailingLlmEvaluator — if the LLM is actually called, it would
        // produce an error. The test verifies the LLM is never consulted for
        // already-denied actions.
        let evaluator = Arc::new(acteon_llm::FailingLlmEvaluator::new("should not be called"));
        let rules = vec![Rule::new("deny-all", Expr::Bool(true), RuleAction::Deny)];
        let gw = build_gateway_with_llm(rules, evaluator, false);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Suppressed { .. }),
            "already-denied action should stay denied, got {outcome:?}"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(
            snap.llm_guardrail_errors, 0,
            "LLM should not have been called"
        );
        assert_eq!(snap.llm_guardrail_allowed, 0);
        assert_eq!(snap.llm_guardrail_denied, 0);
    }

    #[tokio::test]
    async fn llm_guardrail_fail_open() {
        let evaluator = Arc::new(acteon_llm::FailingLlmEvaluator::new("service unavailable"));
        let gw = build_gateway_with_llm(vec![], evaluator, true);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "fail-open should allow action on LLM error, got {outcome:?}"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.llm_guardrail_errors, 1);
    }

    #[tokio::test]
    async fn llm_guardrail_fail_closed() {
        let evaluator = Arc::new(acteon_llm::FailingLlmEvaluator::new("service unavailable"));
        let gw = build_gateway_with_llm(vec![], evaluator, false);

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Suppressed { ref rule } if rule.contains("LLM guardrail")),
            "fail-closed should deny action on LLM error, got {outcome:?}"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.llm_guardrail_errors, 1);
    }

    // -- LLM policy resolution tests ------------------------------------------

    #[tokio::test]
    async fn llm_guardrail_uses_rule_metadata_policy() {
        let evaluator = Arc::new(acteon_llm::CapturingLlmEvaluator::new());
        let mut meta = HashMap::new();
        meta.insert("llm_policy".into(), "Block DROP statements".into());

        let rules =
            vec![Rule::new("guard-sql", Expr::Bool(true), RuleAction::Allow).with_metadata(meta)];

        let gw = build_gateway_with_llm_policies(rules, evaluator.clone(), true, HashMap::new());

        let _ = gw.dispatch(test_action(), None).await.unwrap();
        let policies = evaluator.captured_policies();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0], "Block DROP statements");
    }

    #[tokio::test]
    async fn llm_guardrail_uses_action_type_policy() {
        let evaluator = Arc::new(acteon_llm::CapturingLlmEvaluator::new());
        let mut action_policies = HashMap::new();
        action_policies.insert("send_email".into(), "Block spam content".into());

        // No rules with metadata — should fall through to action-type map.
        let gw = build_gateway_with_llm_policies(vec![], evaluator.clone(), true, action_policies);

        let _ = gw.dispatch(test_action(), None).await.unwrap();
        let policies = evaluator.captured_policies();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0], "Block spam content");
    }

    #[tokio::test]
    async fn llm_guardrail_falls_back_to_global() {
        let evaluator = Arc::new(acteon_llm::CapturingLlmEvaluator::new());

        // No rule metadata, no action-type map — should use global "test policy".
        let gw = build_gateway_with_llm_policies(vec![], evaluator.clone(), true, HashMap::new());

        let _ = gw.dispatch(test_action(), None).await.unwrap();
        let policies = evaluator.captured_policies();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0], "test policy");
    }

    #[tokio::test]
    async fn llm_guardrail_rule_metadata_overrides_action_type() {
        let evaluator = Arc::new(acteon_llm::CapturingLlmEvaluator::new());

        let mut meta = HashMap::new();
        meta.insert("llm_policy".into(), "Rule-level policy".into());
        let rules =
            vec![Rule::new("guard-email", Expr::Bool(true), RuleAction::Allow).with_metadata(meta)];

        let mut action_policies = HashMap::new();
        action_policies.insert("send_email".into(), "Action-type policy".into());

        let gw = build_gateway_with_llm_policies(rules, evaluator.clone(), true, action_policies);

        let _ = gw.dispatch(test_action(), None).await.unwrap();
        let policies = evaluator.captured_policies();
        assert_eq!(policies.len(), 1);
        // Rule metadata should win over action-type policy.
        assert_eq!(policies[0], "Rule-level policy");
    }

    // -- Dry-run tests --------------------------------------------------------

    #[tokio::test]
    async fn dry_run_allow_no_rules() {
        let gw = build_gateway(vec![]);
        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::DryRun {
                verdict,
                matched_rule,
                would_be_provider,
            } => {
                assert_eq!(verdict, "allow");
                assert!(matched_rule.is_none());
                assert_eq!(would_be_provider, "email");
            }
            other => panic!("expected DryRun, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dry_run_suppress() {
        let rules = vec![Rule::new(
            "block-all",
            Expr::Bool(true),
            RuleAction::Suppress,
        )];
        let gw = build_gateway(rules);
        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::DryRun {
                verdict,
                matched_rule,
                would_be_provider,
            } => {
                assert_eq!(verdict, "suppress");
                assert_eq!(matched_rule.as_deref(), Some("block-all"));
                assert_eq!(would_be_provider, "email");
            }
            other => panic!("expected DryRun, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dry_run_reroute() {
        let rules = vec![Rule::new(
            "reroute-sms",
            Expr::Bool(true),
            RuleAction::Reroute {
                target_provider: "sms-fallback".into(),
            },
        )];
        let gw = build_gateway(rules);
        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::DryRun {
                verdict,
                matched_rule,
                would_be_provider,
            } => {
                assert_eq!(verdict, "reroute");
                assert_eq!(matched_rule.as_deref(), Some("reroute-sms"));
                assert_eq!(would_be_provider, "sms-fallback");
            }
            other => panic!("expected DryRun, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dry_run_does_not_execute_provider() {
        let (gw, captured) = build_capturing_gateway(vec![]);
        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::DryRun { .. }));
        // Provider should NOT have been called.
        assert!(captured.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dry_run_does_not_record_dedup_key() {
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

        // Dry-run should return the verdict without recording the dedup key.
        let dry_outcome = gw.dispatch_dry_run(action.clone(), None).await.unwrap();
        match &dry_outcome {
            ActionOutcome::DryRun { verdict, .. } => assert_eq!(verdict, "deduplicate"),
            other => panic!("expected DryRun, got {other:?}"),
        }

        // Normal dispatch should still execute (key was NOT recorded by dry-run).
        let normal_outcome = gw.dispatch(action.clone(), None).await.unwrap();
        assert!(
            matches!(normal_outcome, ActionOutcome::Executed(_)),
            "first normal dispatch should execute because dry-run did not record dedup key"
        );
    }

    #[tokio::test]
    async fn dry_run_batch() {
        let rules = vec![Rule::new(
            "block-all",
            Expr::Bool(true),
            RuleAction::Suppress,
        )];
        let gw = build_gateway(rules);

        let actions = vec![test_action(), test_action()];
        let results = gw.dispatch_batch_dry_run(actions, None).await;
        assert_eq!(results.len(), 2);
        for result in results {
            let outcome = result.unwrap();
            assert!(matches!(outcome, ActionOutcome::DryRun { .. }));
        }
    }

    #[tokio::test]
    async fn dry_run_throttle() {
        let rules = vec![Rule::new(
            "rate-limit",
            Expr::Bool(true),
            RuleAction::Throttle {
                max_count: 100,
                window_seconds: 60,
            },
        )];
        let gw = build_gateway(rules);

        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::DryRun {
                verdict,
                matched_rule,
                would_be_provider,
            } => {
                assert_eq!(verdict, "throttle");
                assert_eq!(matched_rule.as_deref(), Some("rate-limit"));
                assert_eq!(would_be_provider, "email");
            }
            other => panic!("expected DryRun, got {other:?}"),
        }
    }

    // -- Circuit breaker integration tests ------------------------------------

    use crate::circuit_breaker::{CircuitBreakerConfig, CircuitState};

    /// Build a gateway with a failing provider and circuit breaker config.
    fn build_circuit_breaker_gateway(cb_config: CircuitBreakerConfig) -> crate::gateway::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker(cb_config)
            .build()
            .expect("gateway should build")
    }

    /// Build a gateway with a failing primary provider and a healthy fallback,
    /// where the circuit breaker is configured with a fallback provider.
    fn build_circuit_breaker_fallback_gateway(
        cb_config: CircuitBreakerConfig,
    ) -> crate::gateway::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker_provider("email", cb_config)
            .build()
            .expect("gateway should build")
    }

    #[tokio::test]
    async fn circuit_breaker_opens_after_failures_and_rejects() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        };
        let gw = build_circuit_breaker_gateway(config);

        // The provider always fails, so each dispatch records a failure.
        let outcome1 = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome1, ActionOutcome::Failed(_)));

        let outcome2 = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome2, ActionOutcome::Failed(_)));

        // Circuit is now open. Next dispatch should be rejected without
        // calling the provider.
        let outcome3 = gw.dispatch(test_action(), None).await.unwrap();
        match outcome3 {
            ActionOutcome::CircuitOpen {
                provider,
                fallback_chain,
            } => {
                assert_eq!(provider, "email");
                assert!(fallback_chain.is_empty());
            }
            other => panic!("expected CircuitOpen, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert!(snap.circuit_open >= 1);
    }

    #[tokio::test]
    async fn circuit_breaker_uses_fallback_when_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: Some("sms-fallback".into()),
        };
        let gw = build_circuit_breaker_fallback_gateway(config);

        // Trip the circuit with one failure.
        let outcome1 = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome1, ActionOutcome::Failed(_)));

        // Now circuit is open. Next request should be rerouted to fallback.
        let outcome2 = gw.dispatch(test_action(), None).await.unwrap();
        match outcome2 {
            ActionOutcome::Rerouted {
                original_provider,
                new_provider,
                ..
            } => {
                assert_eq!(original_provider, "email");
                assert_eq!(new_provider, "sms-fallback");
            }
            other => panic!("expected Rerouted via fallback, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert!(snap.circuit_fallbacks >= 1);
    }

    #[tokio::test]
    async fn circuit_breaker_records_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(60),
            fallback_provider: None,
        };
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        // Use a healthy provider so executions succeed.
        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(MockProvider::new("email")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker(config)
            .build()
            .expect("gateway should build");

        // Dispatch several successful actions.
        for _ in 0..5 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(matches!(outcome, ActionOutcome::Executed(_)));
        }

        // Circuit should still be closed (successes don't trip it).
        let cb_registry = gw.circuit_breakers().expect("should have circuit breakers");
        let cb = cb_registry.get("email").expect("should have email breaker");
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn circuit_breaker_does_not_affect_unconfigured_providers() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        // Only configure circuit breaker for "sms-fallback", not "email".
        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(MockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            // No circuit breaker configured at all.
            .build()
            .expect("gateway should build");

        // Without circuit breaker, dispatches should always go through.
        for _ in 0..10 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(matches!(outcome, ActionOutcome::Executed(_)));
        }

        assert!(gw.circuit_breakers().is_none());
    }

    #[tokio::test]
    async fn builder_creates_circuit_breakers_for_all_providers() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(MockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .provider(Arc::new(MockProvider::new("webhook")))
            .circuit_breaker(CircuitBreakerConfig::default())
            .build()
            .expect("gateway should build");

        let registry = gw.circuit_breakers().expect("should have registry");
        assert_eq!(registry.len(), 3);
        assert!(registry.get("email").is_some());
        assert!(registry.get("sms-fallback").is_some());
        assert!(registry.get("webhook").is_some());
    }

    #[tokio::test]
    async fn builder_applies_per_provider_override() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let custom = CircuitBreakerConfig {
            failure_threshold: 10,
            success_threshold: 3,
            recovery_timeout: Duration::from_secs(120),
            fallback_provider: Some("webhook".into()),
        };

        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(MockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("webhook")))
            .circuit_breaker(CircuitBreakerConfig::default())
            .circuit_breaker_provider("email", custom)
            .build()
            .expect("gateway should build");

        let registry = gw.circuit_breakers().expect("should have registry");
        let email_cb = registry.get("email").expect("should have email breaker");
        assert_eq!(email_cb.config().failure_threshold, 10);
        assert_eq!(email_cb.config().success_threshold, 3);
        assert_eq!(
            email_cb.config().fallback_provider.as_deref(),
            Some("webhook")
        );

        // Webhook should have default config.
        let webhook_cb = registry
            .get("webhook")
            .expect("should have webhook breaker");
        assert_eq!(webhook_cb.config().failure_threshold, 5); // default
    }

    #[tokio::test]
    async fn circuit_breaker_half_open_allows_probe_then_reopens() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::ZERO, // Immediate transition for testing
            fallback_provider: None,
        };
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        // Use a failing provider to trip the circuit.
        let gw = GatewayBuilder::new()
            .state(store.clone())
            .lock(lock.clone())
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker(config)
            .build()
            .expect("gateway should build");

        // Trip the circuit.
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Failed(_)));

        let registry = gw.circuit_breakers().expect("should have registry");
        let cb = registry.get("email").expect("should have email breaker");
        assert_eq!(cb.state().await, CircuitState::Open);

        // With recovery_timeout=ZERO, the next dispatch transitions to
        // HalfOpen internally, allows the probe (provider still fails),
        // and records the failure -> back to Open.
        let outcome2 = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome2, ActionOutcome::Failed(_)));
        assert_eq!(cb.state().await, CircuitState::Open);

        // While a probe is NOT in flight, the next dispatch can try again.
        // The probe failed above which cleared probe_in_flight, so another
        // dispatch will attempt another probe.
        let outcome3 = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome3, ActionOutcome::Failed(_)));
        assert_eq!(cb.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn circuit_breaker_multiple_providers_independent() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        };

        // email always fails, sms-fallback always succeeds.
        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker(config)
            .build()
            .expect("gateway should build");

        // Trip the email circuit.
        let mut action = test_action();
        let outcome = gw.dispatch(action.clone(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Failed(_)));

        // Email circuit is open.
        let registry = gw.circuit_breakers().expect("should have registry");
        assert_eq!(
            registry.get("email").unwrap().state().await,
            CircuitState::Open
        );

        // sms-fallback circuit should still be closed.
        assert_eq!(
            registry.get("sms-fallback").unwrap().state().await,
            CircuitState::Closed
        );

        // Dispatch to sms-fallback should succeed.
        action.provider = "sms-fallback".into();
        let outcome2 = gw.dispatch(action, None).await.unwrap();
        assert!(matches!(outcome2, ActionOutcome::Executed(_)));

        // sms-fallback circuit still closed.
        assert_eq!(
            registry.get("sms-fallback").unwrap().state().await,
            CircuitState::Closed
        );
    }

    #[tokio::test]
    async fn circuit_breaker_multi_level_fallback() {
        // A→B→C: A and B are open, C is healthy. Should reroute to C.
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("provider-a")))
            .provider(Arc::new(FailingMockProvider::new("provider-b")))
            .provider(Arc::new(MockProvider::new("provider-c")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker_provider(
                "provider-a",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("provider-b".into()),
                },
            )
            .circuit_breaker_provider(
                "provider-b",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("provider-c".into()),
                },
            )
            .build()
            .expect("gateway should build");

        // Trip A's circuit.
        let mut action = test_action();
        action.provider = "provider-a".into();
        let outcome = gw.dispatch(action.clone(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Failed(_)));

        // Trip B's circuit.
        action.provider = "provider-b".into();
        let outcome = gw.dispatch(action.clone(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Failed(_)));

        // Now dispatch to A. A is open, B is open, C is healthy → reroute to C.
        action.provider = "provider-a".into();
        let outcome = gw.dispatch(action, None).await.unwrap();
        match outcome {
            ActionOutcome::Rerouted {
                original_provider,
                new_provider,
                ..
            } => {
                assert_eq!(original_provider, "provider-a");
                assert_eq!(new_provider, "provider-c");
            }
            other => panic!("expected Rerouted to provider-c, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn circuit_breaker_full_chain_exhausted() {
        // A→B→C: all three are open. Should return CircuitOpen with full chain.
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("provider-a")))
            .provider(Arc::new(FailingMockProvider::new("provider-b")))
            .provider(Arc::new(FailingMockProvider::new("provider-c")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker_provider(
                "provider-a",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("provider-b".into()),
                },
            )
            .circuit_breaker_provider(
                "provider-b",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("provider-c".into()),
                },
            )
            .circuit_breaker_provider(
                "provider-c",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: None,
                },
            )
            .build()
            .expect("gateway should build");

        // Trip all three circuits.
        let mut action = test_action();
        for name in ["provider-a", "provider-b", "provider-c"] {
            action.provider = name.into();
            let outcome = gw.dispatch(action.clone(), None).await.unwrap();
            assert!(matches!(outcome, ActionOutcome::Failed(_)));
        }

        // Dispatch to A. Entire chain is open → CircuitOpen.
        action.provider = "provider-a".into();
        let outcome = gw.dispatch(action, None).await.unwrap();
        match outcome {
            ActionOutcome::CircuitOpen {
                provider,
                fallback_chain,
            } => {
                assert_eq!(provider, "provider-a");
                assert_eq!(fallback_chain, vec!["provider-b", "provider-c"]);
            }
            other => panic!("expected CircuitOpen with full chain, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn circuit_breaker_long_fallback_chain() {
        // A→B→C→D: A, B, C open; D healthy. Should reroute to D.
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(FailingMockProvider::new("region-us")))
            .provider(Arc::new(FailingMockProvider::new("region-eu")))
            .provider(Arc::new(FailingMockProvider::new("region-ap")))
            .provider(Arc::new(MockProvider::new("region-backup")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .circuit_breaker_provider(
                "region-us",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("region-eu".into()),
                },
            )
            .circuit_breaker_provider(
                "region-eu",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("region-ap".into()),
                },
            )
            .circuit_breaker_provider(
                "region-ap",
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    success_threshold: 1,
                    recovery_timeout: Duration::from_secs(3600),
                    fallback_provider: Some("region-backup".into()),
                },
            )
            .build()
            .expect("gateway should build");

        // Trip first three circuits.
        let mut action = test_action();
        for name in ["region-us", "region-eu", "region-ap"] {
            action.provider = name.into();
            let outcome = gw.dispatch(action.clone(), None).await.unwrap();
            assert!(matches!(outcome, ActionOutcome::Failed(_)));
        }

        // Dispatch to region-us → should cascade to region-backup.
        action.provider = "region-us".into();
        let outcome = gw.dispatch(action, None).await.unwrap();
        match outcome {
            ActionOutcome::Rerouted {
                original_provider,
                new_provider,
                ..
            } => {
                assert_eq!(original_provider, "region-us");
                assert_eq!(new_provider, "region-backup");
            }
            other => panic!("expected Rerouted to region-backup, got {other:?}"),
        }
    }

    // -- Stream broadcast tests -----------------------------------------------

    use acteon_core::stream::StreamEventType;

    #[tokio::test]
    async fn dispatch_emits_stream_event_on_allow() {
        let gw = build_gateway(vec![]);
        let mut rx = gw.stream_tx().subscribe();

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Executed(_)));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("broadcast should not be closed");

        assert_eq!(event.namespace, "notifications");
        assert_eq!(event.tenant, "tenant-1");
        assert_eq!(event.action_type.as_deref(), Some("send_email"));
        assert!(event.action_id.is_some());
        match event.event_type {
            StreamEventType::ActionDispatched { outcome, provider } => {
                assert_eq!(provider, "email");
                assert!(matches!(outcome, ActionOutcome::Executed(_)));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_emits_stream_event_on_suppress() {
        let rules = vec![Rule::new(
            "block-all",
            Expr::Bool(true),
            RuleAction::Suppress,
        )];
        let gw = build_gateway(rules);
        let mut rx = gw.stream_tx().subscribe();

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("broadcast should not be closed");

        match event.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => {
                assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_emits_stream_event_on_reroute() {
        let rules = vec![Rule::new(
            "reroute-sms",
            Expr::Bool(true),
            RuleAction::Reroute {
                target_provider: "sms-fallback".into(),
            },
        )];
        let gw = build_gateway(rules);
        let mut rx = gw.stream_tx().subscribe();

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Rerouted { .. }));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("broadcast should not be closed");

        match event.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => {
                assert!(matches!(outcome, ActionOutcome::Rerouted { .. }));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_emits_stream_event_on_throttle() {
        let rules = vec![Rule::new(
            "rate-limit",
            Expr::Bool(true),
            RuleAction::Throttle {
                max_count: 100,
                window_seconds: 60,
            },
        )];
        let gw = build_gateway(rules);
        let mut rx = gw.stream_tx().subscribe();

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Throttled { .. }));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("broadcast should not be closed");

        match event.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => {
                assert!(matches!(outcome, ActionOutcome::Throttled { .. }));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    // -- Scheduled action tests ------------------------------------------------

    #[tokio::test]
    async fn dispatch_schedule_returns_scheduled_outcome() {
        let rules = vec![Rule::new(
            "delay-send",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 60 },
        )];
        let gw = build_gateway(rules);
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::Scheduled {
                ref action_id,
                scheduled_for,
            } => {
                assert!(!action_id.is_empty(), "action_id should not be empty");
                // scheduled_for should be roughly 60 seconds in the future
                let now = chrono::Utc::now();
                let diff = (scheduled_for - now).num_seconds();
                assert!(
                    (55..=65).contains(&diff),
                    "scheduled_for should be ~60s in the future, got {diff}s"
                );
            }
            other => panic!("expected Scheduled, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.dispatched, 1);
        assert_eq!(snap.scheduled, 1);
        assert_eq!(snap.executed, 0);
    }

    #[tokio::test]
    async fn dispatch_schedule_stores_state() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());

        let rules = vec![Rule::new(
            "delay-send",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 120 },
        )];

        let gw = GatewayBuilder::new()
            .state(Arc::clone(&store))
            .lock(lock)
            .rules(rules)
            .provider(Arc::new(MockProvider::new("email")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .build()
            .expect("gateway should build");

        let action = test_action();
        let outcome = gw.dispatch(action.clone(), None).await.unwrap();

        let action_id = match &outcome {
            ActionOutcome::Scheduled { action_id, .. } => action_id.clone(),
            other => panic!("expected Scheduled, got {other:?}"),
        };

        // Verify the scheduled action data was stored
        let sched_key = acteon_state::StateKey::new(
            "notifications",
            "tenant-1",
            acteon_state::KeyKind::ScheduledAction,
            &action_id,
        );
        let data = store.get(&sched_key).await.unwrap();
        assert!(
            data.is_some(),
            "scheduled action data should exist in state store"
        );

        let parsed: serde_json::Value = serde_json::from_str(&data.unwrap()).unwrap();
        assert_eq!(parsed["action_id"].as_str().unwrap(), action_id);
        assert!(parsed["action"].is_object());
        assert!(parsed["scheduled_for"].is_string());
        assert!(parsed["created_at"].is_string());

        // Verify the pending scheduled index was stored
        let pending_key = acteon_state::StateKey::new(
            "notifications",
            "tenant-1",
            acteon_state::KeyKind::PendingScheduled,
            &action_id,
        );
        let pending_data = store.get(&pending_key).await.unwrap();
        assert!(
            pending_data.is_some(),
            "pending scheduled index should exist"
        );
    }

    #[tokio::test]
    async fn dispatch_schedule_zero_delay_errors() {
        let rules = vec![Rule::new(
            "bad-delay",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 0 },
        )];
        let gw = build_gateway(rules);
        let result = gw.dispatch(test_action(), None).await;
        assert!(result.is_err(), "zero delay should produce an error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("at least 1 second"),
            "error should mention minimum delay: {err_msg}"
        );
    }

    #[tokio::test]
    async fn dispatch_schedule_exceeds_max_delay_errors() {
        // 8 days > 7 days max
        let rules = vec![Rule::new(
            "too-long",
            Expr::Bool(true),
            RuleAction::Schedule {
                delay_seconds: 8 * 24 * 60 * 60,
            },
        )];
        let gw = build_gateway(rules);
        let result = gw.dispatch(test_action(), None).await;
        assert!(
            result.is_err(),
            "exceeding max delay should produce an error"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("exceeds maximum"),
            "error should mention maximum: {err_msg}"
        );
    }

    #[tokio::test]
    async fn dispatch_schedule_max_boundary_succeeds() {
        // Exactly 7 days should succeed
        let rules = vec![Rule::new(
            "max-delay",
            Expr::Bool(true),
            RuleAction::Schedule {
                delay_seconds: 7 * 24 * 60 * 60,
            },
        )];
        let gw = build_gateway(rules);
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::Scheduled { .. }),
            "exactly 7 days should succeed"
        );
    }

    #[tokio::test]
    async fn dispatch_schedule_prevents_reschedule() {
        let rules = vec![Rule::new(
            "delay",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 60 },
        )];
        let gw = build_gateway(rules);

        // Simulate an action that was already dispatched by the scheduler
        let mut action = test_action();
        let payload = action.payload.as_object_mut().unwrap();
        payload.insert(
            "_scheduled_dispatch".to_string(),
            serde_json::Value::Bool(true),
        );

        let result = gw.dispatch(action, None).await;
        assert!(
            result.is_err(),
            "re-scheduling an already-scheduled action should fail"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("re-schedule"),
            "error should mention re-schedule: {err_msg}"
        );
    }

    #[tokio::test]
    async fn dispatch_schedule_emits_stream_event() {
        let rules = vec![Rule::new(
            "delay-send",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 30 },
        )];
        let gw = build_gateway(rules);
        let mut rx = gw.stream_tx().subscribe();

        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Scheduled { .. }));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("broadcast should not be closed");

        match event.event_type {
            acteon_core::StreamEventType::ActionDispatched { outcome, .. } => {
                assert!(matches!(outcome, ActionOutcome::Scheduled { .. }));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_schedule_unique_action_ids() {
        let rules = vec![Rule::new(
            "delay",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 60 },
        )];
        let gw = build_gateway(rules);

        let mut ids = Vec::new();
        for _ in 0..10 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            if let ActionOutcome::Scheduled { action_id, .. } = outcome {
                ids.push(action_id);
            }
        }
        // All IDs should be unique
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            10,
            "all scheduled action IDs should be unique"
        );
    }

    #[tokio::test]
    async fn dispatch_schedule_metrics_increment() {
        let rules = vec![Rule::new(
            "delay",
            Expr::Bool(true),
            RuleAction::Schedule { delay_seconds: 10 },
        )];
        let gw = build_gateway(rules);

        for _ in 0..5 {
            let _ = gw.dispatch(test_action(), None).await.unwrap();
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.dispatched, 5);
        assert_eq!(snap.scheduled, 5);
        assert_eq!(snap.executed, 0);
        assert_eq!(snap.suppressed, 0);
    }

    #[tokio::test]
    async fn dry_run_does_not_emit_stream_event() {
        let gw = build_gateway(vec![]);
        let mut rx = gw.stream_tx().subscribe();

        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::DryRun { .. }));

        // No event should be emitted for dry-run.
        let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(result.is_err(), "dry-run should not emit a stream event");
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let gw = build_gateway(vec![]);
        let mut rx1 = gw.stream_tx().subscribe();
        let mut rx2 = gw.stream_tx().subscribe();
        let mut rx3 = gw.stream_tx().subscribe();

        let _ = gw.dispatch(test_action(), None).await.unwrap();

        for (i, rx) in [&mut rx1, &mut rx2, &mut rx3].iter_mut().enumerate() {
            let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("subscriber {i} should receive within timeout"))
                .unwrap_or_else(|_| panic!("subscriber {i} should not see closed channel"));
            assert_eq!(event.namespace, "notifications");
        }
    }

    #[tokio::test]
    async fn no_subscriber_does_not_block_dispatch() {
        let gw = build_gateway(vec![]);
        // No subscribers at all -- dispatch should still succeed.
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Executed(_)));
    }

    #[tokio::test]
    async fn stream_event_id_is_unique_per_dispatch() {
        let gw = build_gateway(vec![]);
        let mut rx = gw.stream_tx().subscribe();

        let _ = gw.dispatch(test_action(), None).await.unwrap();
        let _ = gw.dispatch(test_action(), None).await.unwrap();

        let event1 = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let event2 = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_ne!(
            event1.id, event2.id,
            "each stream event should have a unique id"
        );
    }

    #[tokio::test]
    async fn stream_event_carries_correct_action_metadata() {
        let gw = build_gateway(vec![]);
        let mut rx = gw.stream_tx().subscribe();

        let action = Action::new(
            "payments",
            "tenant-42",
            "email",
            "process_payment",
            serde_json::json!({"amount": 100}),
        );
        let action_id = action.id.to_string();

        let _ = gw.dispatch(action, None).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(event.namespace, "payments");
        assert_eq!(event.tenant, "tenant-42");
        assert_eq!(event.action_type.as_deref(), Some("process_payment"));
        assert_eq!(event.action_id.as_deref(), Some(action_id.as_str()));
    }

    #[tokio::test]
    async fn broadcast_lagged_subscriber_gets_error() {
        // Build a gateway with a very small buffer.
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .provider(Arc::new(MockProvider::new("email")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .stream_buffer_size(2)
            .build()
            .expect("gateway should build");

        let mut rx = gw.stream_tx().subscribe();

        // Dispatch more events than the buffer can hold.
        for _ in 0..5 {
            let _ = gw.dispatch(test_action(), None).await.unwrap();
        }

        // The slow subscriber should see a Lagged error.
        let mut saw_lagged = false;
        for _ in 0..5 {
            match rx.try_recv() {
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                    saw_lagged = true;
                    break;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            }
        }
        assert!(saw_lagged, "slow subscriber should experience lagged error");
    }

    // -- resolve_next_step tests -----------------------------------------------

    mod resolve_next_step_tests {
        use std::collections::HashMap;

        use acteon_core::chain::{
            BranchCondition, BranchOperator, ChainConfig, ChainStepConfig, StepResult,
        };
        use chrono::Utc;

        use crate::gateway::Gateway;

        fn make_step_result(success: bool, body: Option<serde_json::Value>) -> StepResult {
            StepResult {
                step_name: "test".into(),
                success,
                response_body: body,
                error: None,
                completed_at: Utc::now(),
            }
        }

        /// Helper to build the step index map from a config (mirrors the cached
        /// map that `Gateway::build()` pre-computes).
        fn index_map(config: &ChainConfig) -> HashMap<String, usize> {
            config.step_index_map()
        }

        #[test]
        fn linear_chain_returns_next_sequential_index() {
            let config = ChainConfig::new("linear")
                .with_step(ChainStepConfig::new("a", "p", "t", serde_json::json!({})))
                .with_step(ChainStepConfig::new("b", "p", "t", serde_json::json!({})))
                .with_step(ChainStepConfig::new("c", "p", "t", serde_json::json!({})));
            let result = make_step_result(true, None);
            let map = index_map(&config);

            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result, &map),
                Some(1)
            );
            assert_eq!(
                Gateway::resolve_next_step(&config, 1, &result, &map),
                Some(2)
            );
        }

        #[test]
        fn branch_condition_matches_returns_target_index() {
            let config = ChainConfig::new("branching")
                .with_step(
                    ChainStepConfig::new("check", "p", "t", serde_json::json!({})).with_branch(
                        BranchCondition::new(
                            "success",
                            BranchOperator::Eq,
                            Some(serde_json::json!(true)),
                            "handle_success",
                        ),
                    ),
                )
                .with_step(ChainStepConfig::new(
                    "handle_failure",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "handle_success",
                    "p",
                    "t",
                    serde_json::json!({}),
                ));

            let result = make_step_result(true, None);
            let map = index_map(&config);
            // Branch matches success==true, so target is "handle_success" at index 2.
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result, &map),
                Some(2)
            );
        }

        #[test]
        fn multiple_branches_first_match_wins() {
            let config = ChainConfig::new("multi-branch")
                .with_step(
                    ChainStepConfig::new("check", "p", "t", serde_json::json!({}))
                        .with_branch(BranchCondition::new(
                            "body.status",
                            BranchOperator::Eq,
                            Some(serde_json::json!("critical")),
                            "escalate",
                        ))
                        .with_branch(BranchCondition::new(
                            "body.status",
                            BranchOperator::Eq,
                            Some(serde_json::json!("warning")),
                            "warn",
                        ))
                        .with_branch(BranchCondition::new(
                            "body.status",
                            BranchOperator::Exists,
                            None,
                            "log",
                        )),
                )
                .with_step(ChainStepConfig::new(
                    "escalate",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "warn",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new("log", "p", "t", serde_json::json!({})));

            let map = index_map(&config);
            // status is "warning" — first branch (critical) does NOT match,
            // second branch (warning) matches, so target is "warn" at index 2.
            let result = make_step_result(true, Some(serde_json::json!({"status": "warning"})));
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result, &map),
                Some(2)
            );

            // status is "info" — no exact match, but "exists" matches, so "log" at index 3.
            let result_info = make_step_result(true, Some(serde_json::json!({"status": "info"})));
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result_info, &map),
                Some(3)
            );
        }

        #[test]
        fn no_branch_matches_uses_default_next() {
            let config = ChainConfig::new("default-next")
                .with_step(
                    ChainStepConfig::new("check", "p", "t", serde_json::json!({}))
                        .with_branch(BranchCondition::new(
                            "body.status",
                            BranchOperator::Eq,
                            Some(serde_json::json!("critical")),
                            "escalate",
                        ))
                        .with_default_next("fallback"),
                )
                .with_step(ChainStepConfig::new(
                    "escalate",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "fallback",
                    "p",
                    "t",
                    serde_json::json!({}),
                ));

            let map = index_map(&config);
            // status is "ok" — branch does not match, so default_next = "fallback" at index 2.
            let result = make_step_result(true, Some(serde_json::json!({"status": "ok"})));
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result, &map),
                Some(2)
            );
        }

        #[test]
        fn no_branch_matches_no_default_next_falls_through_to_sequential() {
            let config = ChainConfig::new("no-default")
                .with_step(
                    ChainStepConfig::new("check", "p", "t", serde_json::json!({})).with_branch(
                        BranchCondition::new(
                            "body.status",
                            BranchOperator::Eq,
                            Some(serde_json::json!("critical")),
                            "escalate",
                        ),
                    ),
                )
                .with_step(ChainStepConfig::new(
                    "next_sequential",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "escalate",
                    "p",
                    "t",
                    serde_json::json!({}),
                ));

            let map = index_map(&config);
            // No branch matches and no default_next — falls through to sequential (index 1).
            let result = make_step_result(true, Some(serde_json::json!({"status": "ok"})));
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result, &map),
                Some(1)
            );
        }

        #[test]
        fn final_step_no_branches_returns_none() {
            let config = ChainConfig::new("linear")
                .with_step(ChainStepConfig::new("a", "p", "t", serde_json::json!({})))
                .with_step(ChainStepConfig::new("b", "p", "t", serde_json::json!({})));

            let result = make_step_result(true, None);
            let map = index_map(&config);
            // At the last step (index 1), sequential next would be 2 which is out of bounds.
            assert_eq!(Gateway::resolve_next_step(&config, 1, &result, &map), None);
        }

        #[test]
        fn final_step_with_branch_to_earlier_step_returns_target() {
            let config = ChainConfig::new("branch-back")
                .with_step(ChainStepConfig::new(
                    "start",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(
                    ChainStepConfig::new("end", "p", "t", serde_json::json!({})).with_branch(
                        BranchCondition::new(
                            "success",
                            BranchOperator::Eq,
                            Some(serde_json::json!(true)),
                            "start",
                        ),
                    ),
                );

            let result = make_step_result(true, None);
            let map = index_map(&config);
            // At the last step, branch matches → jumps back to "start" at index 0.
            assert_eq!(
                Gateway::resolve_next_step(&config, 1, &result, &map),
                Some(0)
            );
        }

        #[test]
        fn branch_with_success_true_evaluates_correctly() {
            let config = ChainConfig::new("success-branch")
                .with_step(
                    ChainStepConfig::new("check", "p", "t", serde_json::json!({}))
                        .with_branch(BranchCondition::new(
                            "success",
                            BranchOperator::Eq,
                            Some(serde_json::json!(true)),
                            "ok_path",
                        ))
                        .with_branch(BranchCondition::new(
                            "success",
                            BranchOperator::Eq,
                            Some(serde_json::json!(false)),
                            "err_path",
                        )),
                )
                .with_step(ChainStepConfig::new(
                    "ok_path",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "err_path",
                    "p",
                    "t",
                    serde_json::json!({}),
                ));

            let map = index_map(&config);
            let success_result = make_step_result(true, None);
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &success_result, &map),
                Some(1),
                "success=true should route to ok_path"
            );

            let failure_result = make_step_result(false, None);
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &failure_result, &map),
                Some(2),
                "success=false should route to err_path"
            );
        }

        #[test]
        fn branch_with_body_field_condition_evaluates_correctly() {
            let config = ChainConfig::new("body-branch")
                .with_step(
                    ChainStepConfig::new("api_call", "p", "t", serde_json::json!({}))
                        .with_branch(BranchCondition::new(
                            "body.result.code",
                            BranchOperator::Eq,
                            Some(serde_json::json!(200)),
                            "process",
                        ))
                        .with_branch(BranchCondition::new(
                            "body.result.code",
                            BranchOperator::Eq,
                            Some(serde_json::json!(404)),
                            "not_found",
                        ))
                        .with_default_next("error_handler"),
                )
                .with_step(ChainStepConfig::new(
                    "process",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "not_found",
                    "p",
                    "t",
                    serde_json::json!({}),
                ))
                .with_step(ChainStepConfig::new(
                    "error_handler",
                    "p",
                    "t",
                    serde_json::json!({}),
                ));

            let map = index_map(&config);
            let result_200 = make_step_result(
                true,
                Some(serde_json::json!({"result": {"code": 200, "data": "ok"}})),
            );
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result_200, &map),
                Some(1),
                "code 200 should route to process"
            );

            let result_404 =
                make_step_result(true, Some(serde_json::json!({"result": {"code": 404}})));
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result_404, &map),
                Some(2),
                "code 404 should route to not_found"
            );

            let result_500 =
                make_step_result(true, Some(serde_json::json!({"result": {"code": 500}})));
            assert_eq!(
                Gateway::resolve_next_step(&config, 0, &result_500, &map),
                Some(3),
                "code 500 should fall through to default_next error_handler"
            );
        }
    }

    // -- Quota tests ----------------------------------------------------------

    fn make_quota_policy(
        namespace: &str,
        tenant: &str,
        max_actions: u64,
        window: acteon_core::QuotaWindow,
        overage_behavior: acteon_core::OverageBehavior,
        enabled: bool,
    ) -> acteon_core::QuotaPolicy {
        acteon_core::QuotaPolicy {
            id: uuid::Uuid::new_v4().to_string(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            max_actions,
            window,
            overage_behavior,
            enabled,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            description: None,
            labels: HashMap::new(),
        }
    }

    fn build_gateway_with_quota(
        policies: Vec<acteon_core::QuotaPolicy>,
    ) -> crate::gateway::Gateway {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(MockProvider::new("email")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .quota_policies(policies)
            .build()
            .expect("gateway should build")
    }

    #[tokio::test]
    async fn quota_blocks_when_exceeded() {
        let policy = make_quota_policy(
            "notifications",
            "tenant-1",
            3,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Block,
            true,
        );
        let gw = build_gateway_with_quota(vec![policy]);

        // First 3 dispatches should succeed.
        for i in 0..3 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(
                matches!(outcome, ActionOutcome::Executed(_)),
                "dispatch {i} should succeed within quota"
            );
        }

        // 4th dispatch should be blocked.
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::QuotaExceeded {
                tenant,
                limit,
                used,
                overage_behavior,
            } => {
                assert_eq!(tenant, "tenant-1");
                assert_eq!(limit, 3);
                assert_eq!(used, 4, "post-increment value after atomic increment");
                assert_eq!(overage_behavior, "block");
            }
            other => panic!("expected QuotaExceeded, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 3);
        assert_eq!(snap.quota_exceeded, 1);
    }

    #[tokio::test]
    async fn quota_warns_when_exceeded() {
        let policy = make_quota_policy(
            "notifications",
            "tenant-1",
            2,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Warn,
            true,
        );
        let gw = build_gateway_with_quota(vec![policy]);

        // Dispatch 3 actions — all should succeed because Warn doesn't block.
        for i in 0..3 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(
                matches!(outcome, ActionOutcome::Executed(_)),
                "dispatch {i} should succeed with Warn overage behavior"
            );
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 3);
        assert_eq!(
            snap.quota_warned, 1,
            "third dispatch should trigger a quota warning"
        );
        assert_eq!(
            snap.quota_exceeded, 0,
            "Warn behavior should not increment exceeded"
        );
    }

    #[tokio::test]
    async fn quota_allows_when_under_limit() {
        let policy = make_quota_policy(
            "notifications",
            "tenant-1",
            10,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Block,
            true,
        );
        let gw = build_gateway_with_quota(vec![policy]);

        // Dispatch 5 actions — all well under the limit of 10.
        for i in 0..5 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(
                matches!(outcome, ActionOutcome::Executed(_)),
                "dispatch {i} should succeed under quota limit"
            );
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 5);
        assert_eq!(snap.quota_exceeded, 0);
        assert_eq!(snap.quota_warned, 0);
    }

    #[tokio::test]
    async fn quota_ignores_disabled_policy() {
        let policy = make_quota_policy(
            "notifications",
            "tenant-1",
            2,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Block,
            false, // disabled
        );
        let gw = build_gateway_with_quota(vec![policy]);

        // Even though max_actions=2, the disabled policy should be ignored.
        for i in 0..5 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(
                matches!(outcome, ActionOutcome::Executed(_)),
                "dispatch {i} should succeed with disabled quota policy"
            );
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 5);
        assert_eq!(snap.quota_exceeded, 0);
    }

    #[tokio::test]
    async fn quota_independent_per_tenant() {
        let policy_a = make_quota_policy(
            "notifications",
            "tenant-a",
            2,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Block,
            true,
        );
        let policy_b = make_quota_policy(
            "notifications",
            "tenant-b",
            3,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Block,
            true,
        );
        let gw = build_gateway_with_quota(vec![policy_a, policy_b]);

        let action_a = || {
            Action::new(
                "notifications",
                "tenant-a",
                "email",
                "send_email",
                serde_json::json!({"to": "a@example.com"}),
            )
        };
        let action_b = || {
            Action::new(
                "notifications",
                "tenant-b",
                "email",
                "send_email",
                serde_json::json!({"to": "b@example.com"}),
            )
        };

        // Tenant A: 2 succeed, 3rd blocked.
        for _ in 0..2 {
            let outcome = gw.dispatch(action_a(), None).await.unwrap();
            assert!(matches!(outcome, ActionOutcome::Executed(_)));
        }
        let outcome_a3 = gw.dispatch(action_a(), None).await.unwrap();
        assert!(
            matches!(outcome_a3, ActionOutcome::QuotaExceeded { .. }),
            "tenant-a third dispatch should be blocked"
        );

        // Tenant B should still have quota remaining — 3 succeed, 4th blocked.
        for _ in 0..3 {
            let outcome = gw.dispatch(action_b(), None).await.unwrap();
            assert!(matches!(outcome, ActionOutcome::Executed(_)));
        }
        let outcome_b4 = gw.dispatch(action_b(), None).await.unwrap();
        assert!(
            matches!(outcome_b4, ActionOutcome::QuotaExceeded { .. }),
            "tenant-b fourth dispatch should be blocked"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 5, "2 from tenant-a + 3 from tenant-b");
        assert_eq!(snap.quota_exceeded, 2, "one block per tenant");
    }

    #[tokio::test]
    async fn quota_degrades_when_exceeded() {
        let policy = make_quota_policy(
            "notifications",
            "tenant-1",
            2,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Degrade {
                fallback_provider: "sms-fallback".into(),
            },
            true,
        );
        // Need both providers registered so the gateway builds cleanly.
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let gw = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .rules(vec![])
            .provider(Arc::new(MockProvider::new("email")))
            .provider(Arc::new(MockProvider::new("sms-fallback")))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(5),
                max_concurrent: 10,
                ..ExecutorConfig::default()
            })
            .quota_policies(vec![policy])
            .build()
            .expect("gateway should build");

        // First 2 dispatches succeed normally.
        for i in 0..2 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(
                matches!(outcome, ActionOutcome::Executed(_)),
                "dispatch {i} should succeed within quota"
            );
        }

        // 3rd dispatch should return QuotaExceeded with degrade behavior.
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        match outcome {
            ActionOutcome::QuotaExceeded {
                tenant,
                limit,
                used,
                overage_behavior,
            } => {
                assert_eq!(tenant, "tenant-1");
                assert_eq!(limit, 2);
                assert_eq!(used, 3, "post-increment value after atomic increment");
                assert_eq!(overage_behavior, "degrade:sms-fallback");
            }
            other => panic!("expected QuotaExceeded with degrade, got {other:?}"),
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 2);
        assert_eq!(snap.quota_degraded, 1);
        assert_eq!(
            snap.quota_exceeded, 0,
            "Degrade uses its own counter, not exceeded"
        );
    }

    #[tokio::test]
    async fn quota_passes_through_when_no_policy() {
        // Build a gateway with no quota policies at all.
        let gw = build_gateway_with_quota(vec![]);

        // All dispatches should succeed — no quota enforcement.
        for i in 0..10 {
            let outcome = gw.dispatch(test_action(), None).await.unwrap();
            assert!(
                matches!(outcome, ActionOutcome::Executed(_)),
                "dispatch {i} should succeed with no quota policy"
            );
        }

        let snap = gw.metrics().snapshot();
        assert_eq!(snap.executed, 10);
        assert_eq!(snap.quota_exceeded, 0);
        assert_eq!(snap.quota_warned, 0);
        assert_eq!(snap.quota_degraded, 0);
    }

    #[tokio::test]
    async fn dry_run_skips_quota_check() {
        let policy = make_quota_policy(
            "notifications",
            "tenant-1",
            1,
            acteon_core::QuotaWindow::Hourly,
            acteon_core::OverageBehavior::Block,
            true,
        );
        let gw = build_gateway_with_quota(vec![policy]);

        // Use up the single allowed action.
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Executed(_)));

        // Normal dispatch should now be blocked.
        let outcome = gw.dispatch(test_action(), None).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::QuotaExceeded { .. }));

        // Dry-run should bypass quota and return DryRun verdict.
        let outcome = gw.dispatch_dry_run(test_action(), None).await.unwrap();
        assert!(
            matches!(outcome, ActionOutcome::DryRun { .. }),
            "dry-run should skip quota check and return DryRun, got {outcome:?}"
        );

        let snap = gw.metrics().snapshot();
        assert_eq!(
            snap.quota_exceeded, 1,
            "only the real dispatch should count"
        );
    }
}
