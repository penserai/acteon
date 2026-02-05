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
use acteon_core::{Action, ActionOutcome, Caller, StateMachineConfig, compute_fingerprint};
use acteon_executor::{ActionExecutor, DeadLetterEntry, DeadLetterSink};
use acteon_provider::ProviderRegistry;
use acteon_rules::{EvalContext, RuleEngine, RuleVerdict};
use acteon_state::{DistributedLock, KeyKind, StateKey, StateStore};

use serde::{Deserialize, Serialize};

use crate::group_manager::GroupManager;

use crate::error::GatewayError;
use crate::metrics::GatewayMetrics;

type HmacSha256 = Hmac<Sha256>;

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
    pub(crate) approval_secret: Vec<u8>,
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
    #[allow(clippy::too_many_lines)]
    pub async fn dispatch(
        &self,
        action: Action,
        caller: Option<&Caller>,
    ) -> Result<ActionOutcome, GatewayError> {
        self.metrics.increment_dispatched();
        let start = std::time::Instant::now();
        let dispatched_at = Utc::now();

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
        let outcome = match &verdict {
            RuleVerdict::Allow => self.execute_action(&action).await,
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
        };

        // 5. Emit audit record (tracked async task for graceful shutdown).
        if let Some(ref audit) = self.audit {
            let record = build_audit_record(
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

        // 6. Release the lock explicitly.
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

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
        use futures::stream::{self, StreamExt};

        // Process actions in parallel with bounded concurrency.
        // The executor already has its own concurrency limits, so we use a
        // reasonable batch concurrency here (e.g., 32 concurrent dispatches).
        const BATCH_CONCURRENCY: usize = 32;

        stream::iter(actions)
            .map(|action| self.dispatch(action, caller))
            .buffer_unordered(BATCH_CONCURRENCY)
            .collect()
            .await
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

    /// Handle the state machine verdict: track event lifecycle.
    #[allow(clippy::too_many_lines)]
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

    /// Compute the HMAC-SHA256 signature for an approval.
    ///
    /// The message uses length-prefixed fields to prevent canonicalization
    /// attacks (e.g., `ns="a:b", tenant="c"` vs `ns="a", tenant="b:c"`).
    /// The `expires_at` timestamp binds the signature to a specific expiry
    /// window so leaked links cannot be replayed after expiration.
    fn compute_approval_sig(&self, ns: &str, tenant: &str, id: &str, expires_at: i64) -> String {
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
        let mut mac =
            HmacSha256::new_from_slice(&self.approval_secret).expect("HMAC accepts any key size");
        mac.update(msg.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Verify the HMAC-SHA256 signature for an approval.
    fn verify_approval_sig(
        &self,
        ns: &str,
        tenant: &str,
        id: &str,
        expires_at: i64,
        sig: &str,
    ) -> bool {
        let expected = self.compute_approval_sig(ns, tenant, id, expires_at);
        // Constant-time comparison
        expected.len() == sig.len()
            && expected
                .bytes()
                .zip(sig.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }

    /// Handle the request approval verdict: store approval record, send notification, return pending.
    #[allow(clippy::too_many_lines)]
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
        let sig = self.compute_approval_sig(
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

        // Store the approval record keyed by namespace:tenant:approval:id
        let approval_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::Approval,
            &id,
        );
        self.state.set(&approval_key, &record_json, ttl).await?;

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
            "{external_url}/v1/approvals/{ns}/{tenant}/{id}/approve?sig={sig}&expires_at={expires_ts}"
        );
        let reject_url = format!(
            "{external_url}/v1/approvals/{ns}/{tenant}/{id}/reject?sig={sig}&expires_at={expires_ts}"
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
    async fn handle_group(
        &self,
        action: &Action,
        group_by: &[String],
        group_wait_seconds: u64,
        _group_interval_seconds: u64,
        _max_group_size: usize,
    ) -> Result<ActionOutcome, GatewayError> {
        let (group_id, group_size, notify_at) = self
            .group_manager
            .add_to_group(action, group_by, group_wait_seconds, self.state.as_ref())
            .await?;

        Ok(ActionOutcome::Grouped {
            group_id,
            group_size,
            notify_at,
        })
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
    ) -> Result<ActionOutcome, GatewayError> {
        // 1. Verify HMAC signature (includes expires_at to prevent replay after expiry)
        if !self.verify_approval_sig(namespace, tenant, id, expires_at, sig) {
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
        let val = self
            .state
            .get(&approval_key)
            .await?
            .ok_or(GatewayError::ApprovalNotFound)?;
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
        self.state.set(&approval_key, &updated_json, None).await?;

        // 5. TOCTOU: re-evaluate rules against the stored action
        let action = &record.action;
        let eval_ctx = EvalContext::new(action, self.state.as_ref(), &self.environment);
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
    ) -> Result<(), GatewayError> {
        // 1. Verify HMAC signature (includes expires_at to prevent replay after expiry)
        if !self.verify_approval_sig(namespace, tenant, id, expires_at, sig) {
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
        let val = self
            .state
            .get(&approval_key)
            .await?
            .ok_or(GatewayError::ApprovalNotFound)?;
        let record: ApprovalRecord = serde_json::from_str(&val)
            .map_err(|e| GatewayError::Configuration(format!("corrupt approval record: {e}")))?;

        if record.status != "pending" {
            return Err(GatewayError::ApprovalAlreadyDecided(record.status));
        }

        // 4. Update status to "rejected"
        let mut updated = record;
        updated.status = "rejected".to_string();
        updated.decided_at = Some(Utc::now());
        let updated_json = serde_json::to_string(&updated).map_err(|e| {
            GatewayError::Configuration(format!("failed to serialize approval: {e}"))
        })?;
        self.state.set(&approval_key, &updated_json, None).await?;

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
        let val = self
            .state
            .get(&approval_key)
            .await?
            .ok_or(GatewayError::ApprovalNotFound)?;
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
        let sig = self.compute_approval_sig(namespace, tenant, id, expires_ts);

        let external_url = self
            .external_url
            .as_deref()
            .unwrap_or("http://localhost:8080");
        let approve_url = format!(
            "{external_url}/v1/approvals/{namespace}/{tenant}/{id}/approve?sig={sig}&expires_at={expires_ts}"
        );
        let reject_url = format!(
            "{external_url}/v1/approvals/{namespace}/{tenant}/{id}/reject?sig={sig}&expires_at={expires_ts}"
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
        let eval_ctx = EvalContext::new(&record.action, self.state.as_ref(), &self.environment);
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

        // Preserve the original TTL by computing remaining time
        let remaining = updated.expires_at - Utc::now();
        #[allow(clippy::cast_sign_loss)]
        let ttl = if remaining.num_seconds() > 0 {
            Some(Duration::from_secs(remaining.num_seconds() as u64))
        } else {
            None
        };
        self.state.set(&approval_key, &updated_json, ttl).await?;

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
    ) -> Result<Option<ApprovalStatus>, GatewayError> {
        // Verify HMAC signature
        if !self.verify_approval_sig(namespace, tenant, id, expires_at, sig) {
            return Ok(None);
        }

        let approval_key = StateKey::new(namespace, tenant, KeyKind::Approval, id);
        let Some(val) = self.state.get(&approval_key).await? else {
            return Ok(None);
        };

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
        for (_key, val) in entries {
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
}

// -- Audit helpers -----------------------------------------------------------

/// Extract a string tag from a `RuleVerdict`.
fn verdict_tag(verdict: &RuleVerdict) -> &'static str {
    match verdict {
        RuleVerdict::Allow => "allow",
        RuleVerdict::Deny(_) => "deny",
        RuleVerdict::Deduplicate { .. } => "deduplicate",
        RuleVerdict::Suppress(_) => "suppress",
        RuleVerdict::Reroute { .. } => "reroute",
        RuleVerdict::Throttle { .. } => "throttle",
        RuleVerdict::Modify { .. } => "modify",
        RuleVerdict::StateMachine { .. } => "state_machine",
        RuleVerdict::Group { .. } => "group",
        RuleVerdict::RequestApproval { .. } => "request_approval",
    }
}

/// Extract the matched rule name from a `RuleVerdict`, if any.
fn matched_rule_name(verdict: &RuleVerdict) -> Option<String> {
    match verdict {
        RuleVerdict::Allow | RuleVerdict::Deduplicate { .. } => None,
        RuleVerdict::Deny(rule)
        | RuleVerdict::Suppress(rule)
        | RuleVerdict::Reroute { rule, .. }
        | RuleVerdict::Throttle { rule, .. }
        | RuleVerdict::Modify { rule, .. }
        | RuleVerdict::StateMachine { rule, .. }
        | RuleVerdict::Group { rule, .. }
        | RuleVerdict::RequestApproval { rule, .. } => Some(rule.clone()),
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
    }
}

/// Build an `AuditRecord` from the dispatch context.
#[allow(clippy::too_many_arguments)]
fn build_audit_record(
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
    };

    AuditRecord {
        id: uuid::Uuid::new_v4().to_string(),
        action_id: action.id.to_string(),
        namespace: action.namespace.to_string(),
        tenant: action.tenant.to_string(),
        provider: action.provider.to_string(),
        action_type: action.action_type.clone(),
        verdict: verdict_tag(verdict).to_owned(),
        matched_rule: matched_rule_name(verdict),
        outcome: outcome_tag(outcome).to_owned(),
        action_payload,
        verdict_details: serde_json::json!({ "verdict": verdict_tag(verdict) }),
        outcome_details,
        metadata: serde_json::to_value(&action.metadata).unwrap_or_default(),
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
}
