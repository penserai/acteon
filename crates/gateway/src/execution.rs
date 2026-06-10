//! Durable-execution support: per-execution event history, external
//! signals, search attributes, and cross-chain visibility queries.
//!
//! The history log lives in the state store under
//! `KeyKind::Custom("exec_history")` keyed by execution ID and is shared by
//! task chains and workflow executions, so `GET /v1/executions/{id}/history`
//! works uniformly for both.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::{debug, warn};

use acteon_core::chain::WaitState;
use acteon_core::{ChainState, ChainStatus, ExecutionEventType, ExecutionHistory};
use acteon_state::{KeyKind, StateKey};

use crate::error::GatewayError;
use crate::gateway::Gateway;

/// State-store kind for execution history logs.
pub(crate) const EXEC_HISTORY_KIND: &str = "exec_history";
/// State-store kind for buffered chain signals.
pub(crate) const CHAIN_SIGNAL_KIND: &str = "chain_signal";

/// How long a buffered (not-yet-consumed) signal is retained.
const SIGNAL_BUFFER_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Filters for listing executions across chains.
#[derive(Debug, Default, Clone)]
pub struct ExecutionFilter {
    /// Only executions of this chain definition.
    pub chain_name: Option<String>,
    /// Only executions in this status.
    pub status: Option<ChainStatus>,
    /// Only executions started at or after this time.
    pub started_after: Option<DateTime<Utc>>,
    /// Only executions started at or before this time.
    pub started_before: Option<DateTime<Utc>>,
    /// Only executions whose search attributes contain all of these
    /// key/value pairs (string comparison).
    pub attributes: Vec<(String, String)>,
    /// Maximum number of executions to return (default 200).
    pub limit: Option<usize>,
}

impl Gateway {
    /// Append an event to an execution's history log. Best-effort: failures
    /// are logged but never propagate, so a history outage cannot fail the
    /// execution itself.
    ///
    /// `ttl` is applied to the history key when set (used on terminal events
    /// so history expires together with the execution state).
    pub async fn append_execution_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        event: ExecutionEventType,
        ttl: Option<Duration>,
    ) {
        let key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(EXEC_HISTORY_KIND.into()),
            execution_id,
        );

        let mut history = match self.load_execution_history(&key).await {
            Ok(history) => history,
            Err(e) => {
                warn!(execution_id, error = %e, "failed to load execution history; event dropped");
                return;
            }
        };

        if !history.append(event) {
            warn!(
                execution_id,
                "execution history at capacity; non-terminal event dropped"
            );
            return;
        }

        let json = match serde_json::to_string(&history) {
            Ok(json) => json,
            Err(e) => {
                warn!(execution_id, error = %e, "failed to serialize execution history");
                return;
            }
        };
        let stored = match self.encrypt_state_value(&json) {
            Ok(v) => v,
            Err(e) => {
                warn!(execution_id, error = %e, "failed to encrypt execution history");
                return;
            }
        };
        if let Err(e) = self.state.set(&key, &stored, ttl).await {
            warn!(execution_id, error = %e, "failed to persist execution history");
        }
    }

    /// Load the history log for an execution. Returns an empty history when
    /// none has been recorded.
    pub async fn get_execution_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<ExecutionHistory, GatewayError> {
        let key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(EXEC_HISTORY_KIND.into()),
            execution_id,
        );
        self.load_execution_history(&key).await
    }

    async fn load_execution_history(
        &self,
        key: &StateKey,
    ) -> Result<ExecutionHistory, GatewayError> {
        match self.state.get(key).await? {
            Some(raw) => {
                let json = self.decrypt_state_value(&raw)?;
                serde_json::from_str(&json).map_err(|e| {
                    GatewayError::ChainError(format!(
                        "failed to deserialize execution history: {e}"
                    ))
                })
            }
            None => Ok(ExecutionHistory::default()),
        }
    }

    /// Deliver an external signal to a chain execution.
    ///
    /// The signal is buffered durably; if the chain is currently paused on a
    /// `wait_for_signal` step for this signal it is woken immediately,
    /// otherwise the buffered signal is consumed when the chain reaches the
    /// wait step.
    pub async fn signal_chain(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        signal_name: &str,
        payload: serde_json::Value,
    ) -> Result<(), GatewayError> {
        if signal_name.is_empty() {
            return Err(GatewayError::ChainError(
                "signal name must not be empty".into(),
            ));
        }

        let lock_name = format!("chain:{chain_id}");
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let result = self
            .signal_chain_locked(namespace, tenant, chain_id, signal_name, payload)
            .await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }

    async fn signal_chain_locked(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        signal_name: &str,
        payload: serde_json::Value,
    ) -> Result<(), GatewayError> {
        let chain_state = self
            .get_chain_status(namespace, tenant, chain_id)
            .await?
            .ok_or_else(|| GatewayError::ChainError(format!("chain not found: {chain_id}")))?;

        if !chain_state.status.is_active() {
            return Err(GatewayError::ChainError(format!(
                "chain is not active (status: {:?})",
                chain_state.status
            )));
        }

        // Buffer the signal durably so it is consumed even if the chain has
        // not reached the wait step yet.
        let buffer_key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(CHAIN_SIGNAL_KIND.into()),
            format!("{chain_id}:{signal_name}"),
        );
        let payload_json = serde_json::to_string(&payload).map_err(|e| {
            GatewayError::ChainError(format!("failed to serialize signal payload: {e}"))
        })?;
        self.state
            .set(&buffer_key, &payload_json, Some(SIGNAL_BUFFER_TTL))
            .await?;

        self.append_execution_history(
            namespace,
            tenant,
            chain_id,
            ExecutionEventType::SignalReceived {
                signal_name: signal_name.to_owned(),
                payload,
            },
            None,
        )
        .await;

        // Wake the chain only when it is paused waiting for this signal.
        // (Waking a timer wait would be a no-op re-index; waking a running
        // chain is unnecessary — the buffered signal is consumed when the
        // wait step is reached.)
        let waiting_for_this_signal = matches!(
            &chain_state.wait_state,
            Some(WaitState::Signal { signal_name: awaited, .. }) if awaited == signal_name
        ) && chain_state.status == ChainStatus::WaitingSignal;

        if waiting_for_this_signal {
            let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingChains, chain_id);
            self.state
                .index_chain_ready(&pending_key, Utc::now().timestamp_millis())
                .await?;
            debug!(chain_id, signal_name, "signal delivered; chain woken");
        } else {
            debug!(
                chain_id,
                signal_name, "signal buffered for later consumption"
            );
        }

        Ok(())
    }

    /// Consume a buffered signal for a chain, if present. Returns the signal
    /// payload and removes the buffer entry.
    pub(crate) async fn take_buffered_signal(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        signal_name: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let buffer_key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(CHAIN_SIGNAL_KIND.into()),
            format!("{chain_id}:{signal_name}"),
        );
        match self.state.get(&buffer_key).await? {
            Some(raw) => {
                let payload = serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
                let _ = self.state.delete(&buffer_key).await;
                Ok(Some(payload))
            }
            None => Ok(None),
        }
    }

    /// Merge search attributes into an execution. Existing keys are
    /// overwritten; other keys are preserved.
    pub async fn upsert_search_attributes(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        attributes: HashMap<String, serde_json::Value>,
    ) -> Result<ChainState, GatewayError> {
        let lock_name = format!("chain:{chain_id}");
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let result: Result<ChainState, GatewayError> = async {
            let mut chain_state = self
                .get_chain_status(namespace, tenant, chain_id)
                .await?
                .ok_or_else(|| GatewayError::ChainError(format!("chain not found: {chain_id}")))?;

            chain_state
                .search_attributes
                .extend(attributes.iter().map(|(k, v)| (k.clone(), v.clone())));
            chain_state.updated_at = Utc::now();

            let ttl = if chain_state.status.is_active() {
                None
            } else {
                self.completed_chain_ttl
            };
            let chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, chain_id);
            let json = serde_json::to_string(&chain_state).map_err(|e| {
                GatewayError::ChainError(format!("failed to serialize chain state: {e}"))
            })?;
            let stored = self.encrypt_state_value(&json)?;
            self.state.set(&chain_key, &stored, ttl).await?;

            self.append_execution_history(
                namespace,
                tenant,
                chain_id,
                ExecutionEventType::SearchAttributesUpserted { attributes },
                None,
            )
            .await;

            Ok(chain_state)
        }
        .await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }

    /// Reset a chain execution to re-run from an earlier step.
    ///
    /// Works on terminal executions (completed, failed, cancelled, timed
    /// out) and on paused ones — any in-flight wait is abandoned (a pending
    /// worker task is cancelled best-effort). Step results from the reset
    /// point onward are discarded; results of steps executed *before* the
    /// target step on the recorded execution path are preserved, so
    /// `{{steps.NAME.*}}` templates keep resolving.
    ///
    /// The target step must exist in the execution's pinned definition and
    /// must have been reached by the original run.
    #[allow(clippy::too_many_lines)]
    pub async fn reset_execution(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        target_step: &str,
        reason: Option<String>,
    ) -> Result<ChainState, GatewayError> {
        let lock_name = format!("chain:{chain_id}");
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(30), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let result: Result<ChainState, GatewayError> = async {
            let mut chain_state = self
                .get_chain_status(namespace, tenant, chain_id)
                .await?
                .ok_or_else(|| GatewayError::ChainError(format!("chain not found: {chain_id}")))?;

            let chain_config = chain_state
                .config_snapshot
                .as_deref()
                .cloned()
                .or_else(|| self.chains.read().get(&chain_state.chain_name).cloned())
                .ok_or_else(|| {
                    GatewayError::ChainError(format!(
                        "chain configuration not found: {}",
                        chain_state.chain_name
                    ))
                })?;

            let target_idx = chain_config
                .steps
                .iter()
                .position(|s| s.name == target_step)
                .ok_or_else(|| {
                    GatewayError::ChainError(format!(
                        "step `{target_step}` not found in chain `{}`",
                        chain_state.chain_name
                    ))
                })?;

            let reached = chain_state.execution_path.iter().any(|n| n == target_step)
                || target_idx <= chain_state.current_step;
            if !reached {
                return Err(GatewayError::ChainError(format!(
                    "cannot reset to step `{target_step}`: the execution never reached it"
                )));
            }

            // Abandon any in-flight wait. A pending worker task is cancelled
            // so a late completion can't race the reset (the resume hook
            // also checks the wait state, so this is belt-and-braces).
            if let Some(WaitState::Worker { task_id, .. }) = &chain_state.wait_state {
                let task_id = task_id.clone();
                let _ = self.cancel_worker_task(namespace, tenant, &task_id).await;
            }
            chain_state.wait_state = None;

            // Keep only the part of the execution path strictly before the
            // first occurrence of the target step; results of those steps
            // are preserved, everything else is cleared for re-execution.
            let cut = chain_state
                .execution_path
                .iter()
                .position(|n| n == target_step)
                .unwrap_or(chain_state.execution_path.len());
            chain_state.execution_path.truncate(cut);
            let kept: std::collections::HashSet<&str> = chain_state
                .execution_path
                .iter()
                .map(String::as_str)
                .collect();
            for (i, step) in chain_config.steps.iter().enumerate() {
                if kept.contains(step.name.as_str()) {
                    continue;
                }
                if let Some(slot) = chain_state.step_results.get_mut(i) {
                    *slot = None;
                }
                if let Some(attempts) = chain_state.step_attempts.get_mut(i) {
                    *attempts = 0;
                }
                if let Some(history) = chain_state.step_history.get_mut(i) {
                    history.clear();
                }
            }

            let now = Utc::now();
            chain_state.execution_path.push(target_step.to_owned());
            chain_state.current_step = target_idx;
            chain_state.status = ChainStatus::Running;
            chain_state.cancel_reason = None;
            chain_state.cancelled_by = None;
            chain_state.updated_at = now;
            // An already-expired deadline would immediately time the
            // execution out again; restart the timeout window instead.
            #[allow(clippy::cast_possible_wrap)]
            if let (Some(expires_at), Some(timeout)) =
                (chain_state.expires_at, chain_config.timeout_seconds)
                && expires_at <= now
            {
                chain_state.expires_at = Some(now + chrono::Duration::seconds(timeout as i64));
            }

            let chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, chain_id);
            let json = serde_json::to_string(&chain_state).map_err(|e| {
                GatewayError::ChainError(format!("failed to serialize chain state: {e}"))
            })?;
            let stored = self.encrypt_state_value(&json)?;
            self.state.set(&chain_key, &stored, None).await?;

            // Terminal executions were removed from the pending index;
            // re-register so the background advancer drives the re-run.
            let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingChains, chain_id);
            let pending_val = serde_json::json!({
                "chain_id": chain_id,
                "chain_name": chain_state.chain_name,
                "started_at": chain_state.started_at.to_rfc3339(),
            });
            self.state
                .set(&pending_key, &pending_val.to_string(), None)
                .await?;
            self.state
                .index_chain_ready(&pending_key, now.timestamp_millis())
                .await?;

            self.append_execution_history(
                namespace,
                tenant,
                chain_id,
                ExecutionEventType::ExecutionReset {
                    step_name: target_step.to_owned(),
                    step_index: target_idx,
                    reason,
                },
                None,
            )
            .await;

            debug!(
                chain_id,
                target_step, target_idx, "execution reset; re-running from step"
            );
            Ok(chain_state)
        }
        .await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }

    /// List chain executions for visibility queries, including terminal
    /// executions (unlike [`Gateway::list_chains`], which only scans the
    /// pending index).
    ///
    /// Results are sorted most-recently-started first and capped at
    /// `filter.limit` (default 200).
    pub async fn list_executions(
        &self,
        namespace: &str,
        tenant: &str,
        filter: &ExecutionFilter,
    ) -> Result<Vec<ChainState>, GatewayError> {
        let entries = self
            .state
            .scan_keys(namespace, tenant, KeyKind::Chain, None)
            .await?;

        let limit = filter.limit.unwrap_or(200);
        let mut executions = Vec::new();
        for (_, raw) in entries {
            let Ok(json) = self.decrypt_state_value(&raw) else {
                continue;
            };
            let Ok(state) = serde_json::from_str::<ChainState>(&json) else {
                continue;
            };
            if let Some(ref name) = filter.chain_name
                && &state.chain_name != name
            {
                continue;
            }
            if let Some(ref status) = filter.status
                && &state.status != status
            {
                continue;
            }
            if let Some(after) = filter.started_after
                && state.started_at < after
            {
                continue;
            }
            if let Some(before) = filter.started_before
                && state.started_at > before
            {
                continue;
            }
            let attrs_match = filter.attributes.iter().all(|(k, expected)| {
                state.search_attributes.get(k).is_some_and(|v| match v {
                    serde_json::Value::String(s) => s == expected,
                    other => &other.to_string() == expected,
                })
            });
            if !attrs_match {
                continue;
            }
            executions.push(state);
        }

        executions.sort_by_key(|e| std::cmp::Reverse(e.started_at));
        executions.truncate(limit);
        Ok(executions)
    }
}
