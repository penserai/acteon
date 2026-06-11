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

/// State-store kind for execution history event entries.
pub(crate) const EXEC_HISTORY_KIND: &str = "exec_history";
/// State-store kind for the per-execution history sequence counter.
pub(crate) const EXEC_HISTORY_SEQ_KIND: &str = "exec_history_seq";
/// State-store kind for buffered chain signals.
pub(crate) const CHAIN_SIGNAL_KIND: &str = "chain_signal";
/// State-store kind for pinned (immutable) chain definitions, keyed
/// `{name}@{version}` within a namespace/tenant. Written once when the
/// first execution pins a version; every execution of that version
/// resolves it from here instead of embedding a snapshot per execution.
pub(crate) const PINNED_CHAIN_DEF_KIND: &str = "chain_def_pinned";

/// Size cap for the process-local pinned-definition cache; reaching it
/// clears the cache (entries are immutable and re-loadable, so eviction
/// correctness is trivial).
const PINNED_CONFIG_CACHE_CAP: usize = 256;

/// How long a buffered (not-yet-consumed) signal is retained.
const SIGNAL_BUFFER_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

fn signal_buffer_key(namespace: &str, tenant: &str, chain_id: &str, signal_name: &str) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(CHAIN_SIGNAL_KIND.into()),
        format!("{chain_id}:{signal_name}"),
    )
}

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
    /// Storage is append-only: each event lives under its own key
    /// (`{execution_id}:{seq:08}`) with the sequence allocated by an atomic
    /// counter, so appends are O(1) regardless of history length. `ttl` is
    /// applied to the whole history (every event key + the counter) when
    /// set — used on terminal events so history expires together with the
    /// execution state.
    pub async fn append_execution_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        event: ExecutionEventType,
        ttl: Option<Duration>,
    ) {
        let counter_key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(EXEC_HISTORY_SEQ_KIND.into()),
            execution_id,
        );
        let seq = match self.state.increment(&counter_key, 1, None).await {
            Ok(seq) => seq,
            Err(e) => {
                warn!(execution_id, error = %e, "failed to allocate history sequence; event dropped");
                return;
            }
        };
        #[allow(clippy::cast_sign_loss)]
        let event_id = seq.max(1) as u64;

        if event_id > acteon_core::MAX_HISTORY_EVENTS as u64 && !event.is_terminal() {
            warn!(
                execution_id,
                "execution history at capacity; non-terminal event dropped"
            );
            return;
        }

        let entry = acteon_core::ExecutionEvent {
            event_id,
            timestamp: Utc::now(),
            event,
        };
        let json = match serde_json::to_string(&entry) {
            Ok(json) => json,
            Err(e) => {
                warn!(execution_id, error = %e, "failed to serialize execution history event");
                return;
            }
        };
        let stored = match self.encrypt_state_value(&json) {
            Ok(v) => v,
            Err(e) => {
                warn!(execution_id, error = %e, "failed to encrypt execution history event");
                return;
            }
        };
        let event_key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(EXEC_HISTORY_KIND.into()),
            format!("{execution_id}:{event_id:08}"),
        );
        if let Err(e) = self.state.set(&event_key, &stored, None).await {
            warn!(execution_id, error = %e, "failed to persist execution history event");
        }

        // A terminal event seals the history: apply the TTL to every event
        // key and the counter so the log expires with the execution. This
        // is O(events) exactly once per execution.
        if let Some(ttl) = ttl {
            if let Ok(entries) = self
                .state
                .scan_keys(
                    namespace,
                    tenant,
                    KeyKind::Custom(EXEC_HISTORY_KIND.into()),
                    Some(&format!("{execution_id}:")),
                )
                .await
            {
                for (canonical, value) in entries {
                    if let Some(id) = canonical.splitn(4, ':').nth(3) {
                        let key = StateKey::new(
                            namespace,
                            tenant,
                            KeyKind::Custom(EXEC_HISTORY_KIND.into()),
                            id,
                        );
                        let _ = self.state.set(&key, &value, Some(ttl)).await;
                    }
                }
            }
            let _ = self
                .state
                .set(&counter_key, &seq.to_string(), Some(ttl))
                .await;
        }
    }

    /// Load the history log for an execution (events ordered by sequence).
    /// Returns an empty history when none has been recorded.
    pub async fn get_execution_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<ExecutionHistory, GatewayError> {
        let entries = self
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(EXEC_HISTORY_KIND.into()),
                Some(&format!("{execution_id}:")),
            )
            .await?;

        let mut events = Vec::with_capacity(entries.len());
        for (_, raw) in entries {
            let json = self.decrypt_state_value(&raw)?;
            let event: acteon_core::ExecutionEvent = serde_json::from_str(&json).map_err(|e| {
                GatewayError::ChainError(format!("failed to deserialize execution history: {e}"))
            })?;
            events.push(event);
        }
        events.sort_by_key(|e| e.event_id);
        Ok(ExecutionHistory { events })
    }

    /// Persist the definition an execution pins at start. Write-once per
    /// `{name}@{version}`: definitions are immutable per version, so a
    /// concurrent start of the same version is a no-op.
    ///
    /// Pinned definitions are never expired: an execution may sleep for
    /// months and must still resolve the version it started with.
    pub(crate) async fn pin_chain_definition(
        &self,
        namespace: &str,
        tenant: &str,
        config: &acteon_core::ChainConfig,
    ) -> Result<(), GatewayError> {
        let key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(PINNED_CHAIN_DEF_KIND.into()),
            format!("{}@{}", config.name, config.version),
        );
        let json = serde_json::to_string(config).map_err(|e| {
            GatewayError::ChainError(format!("failed to serialize chain definition: {e}"))
        })?;
        let stored = self.encrypt_state_value(&json)?;
        self.state.check_and_set(&key, &stored, None).await?;
        Ok(())
    }

    /// Garbage-collect pinned chain definitions that nothing can resolve
    /// anymore. Returns the number of entries deleted.
    ///
    /// A pinned `{name}@{version}` entry is deleted only when **both** hold:
    ///
    /// - no chain state (active, or terminal but not yet expired) in that
    ///   namespace/tenant still references `(name, version)` — terminal
    ///   states keep resolving their definition for detail/history
    ///   endpoints until their TTL reaps them, so they count as references;
    /// - the version is older than `current - 1` for the registry's
    ///   definition of that name. New executions always pin the *current*
    ///   registry version, so keeping the latest version closes the
    ///   pin-then-persist window (a pin written mid-GC is never a delete
    ///   candidate); keeping `current - 1` additionally covers an execution
    ///   that read the definition just before an update and persists its
    ///   first state just after the reference scan.
    ///
    /// Conservative on read failures: if any chain state cannot be
    /// decrypted or parsed, the cycle aborts without deleting anything —
    /// that record might be the only reference to a candidate.
    pub async fn gc_pinned_definitions(&self) -> Result<usize, GatewayError> {
        let pinned = self
            .state
            .scan_keys_by_kind(KeyKind::Custom(PINNED_CHAIN_DEF_KIND.into()))
            .await?;
        if pinned.is_empty() {
            return Ok(0);
        }

        // Read the registry versions BEFORE the reference scan: a version
        // that becomes non-current during the scan stays protected for
        // this cycle and is reconsidered on the next one.
        let current_versions: HashMap<String, u64> = self
            .chains
            .read()
            .iter()
            .map(|(name, config)| (name.clone(), config.version))
            .collect();

        // Reference set: every (namespace, tenant, name, version) some
        // chain state still resolves.
        let mut referenced: std::collections::HashSet<(String, String, String, u64)> =
            std::collections::HashSet::new();
        for (key, raw) in self.state.scan_keys_by_kind(KeyKind::Chain).await? {
            // Canonical key: {namespace}:{tenant}:chain:{chain_id}
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }
            let json = self.decrypt_state_value(&raw).map_err(|e| {
                GatewayError::ChainError(format!(
                    "pinned-definition GC aborted: failed to decrypt chain state {key}: {e}"
                ))
            })?;
            let value: serde_json::Value = serde_json::from_str(&json).map_err(|e| {
                GatewayError::ChainError(format!(
                    "pinned-definition GC aborted: failed to parse chain state {key}: {e}"
                ))
            })?;
            let Some(name) = value.get("chain_name").and_then(|v| v.as_str()) else {
                continue;
            };
            // Pre-versioning states deserialize with the version default.
            let version = value
                .get("chain_version")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);
            referenced.insert((
                parts[0].to_owned(),
                parts[1].to_owned(),
                name.to_owned(),
                version,
            ));
        }

        let mut deleted = 0usize;
        for (key, _) in pinned {
            // Canonical key: {namespace}:{tenant}:chain_def_pinned:{name}@{version}
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }
            let (namespace, tenant, id) = (parts[0], parts[1], parts[3]);
            // The version is numeric and names may contain `@`, so split on
            // the LAST `@`. Unparseable ids are kept (never delete what we
            // don't understand).
            let Some((name, version)) = id.rsplit_once('@') else {
                continue;
            };
            let Ok(version) = version.parse::<u64>() else {
                continue;
            };
            if let Some(&current) = current_versions.get(name)
                && version.saturating_add(1) >= current
            {
                continue; // current or current-1 (or ahead of the registry)
            }
            if referenced.contains(&(
                namespace.to_owned(),
                tenant.to_owned(),
                name.to_owned(),
                version,
            )) {
                continue;
            }
            let state_key = StateKey::new(
                namespace,
                tenant,
                KeyKind::Custom(PINNED_CHAIN_DEF_KIND.into()),
                id,
            );
            match self.state.delete(&state_key).await {
                Ok(_) => {
                    deleted += 1;
                    self.pinned_config_cache.write().remove(&(
                        namespace.to_owned(),
                        tenant.to_owned(),
                        name.to_owned(),
                        version,
                    ));
                }
                Err(e) => {
                    warn!(key = %key, error = %e, "failed to delete unreferenced pinned definition");
                }
            }
        }
        if deleted > 0 {
            debug!(deleted, "pinned-definition GC removed unreferenced entries");
        }
        Ok(deleted)
    }

    /// Resolve the definition an execution runs against, in order:
    /// process-local cache → pinned-definition store → the legacy snapshot
    /// embedded in pre-store executions → the live registry (pre-pinning
    /// executions only).
    pub async fn execution_config(
        &self,
        chain_state: &ChainState,
    ) -> Result<Option<acteon_core::ChainConfig>, GatewayError> {
        // Legacy executions (created before the pinned store) carry the
        // snapshot inline; honor it verbatim.
        if let Some(snapshot) = chain_state.config_snapshot.as_deref() {
            return Ok(Some(snapshot.clone()));
        }

        let cache_key = (
            chain_state.namespace.clone(),
            chain_state.tenant.clone(),
            chain_state.chain_name.clone(),
            chain_state.chain_version,
        );
        if let Some(config) = self.pinned_config_cache.read().get(&cache_key) {
            return Ok(Some((**config).clone()));
        }

        let key = StateKey::new(
            chain_state.namespace.as_str(),
            chain_state.tenant.as_str(),
            KeyKind::Custom(PINNED_CHAIN_DEF_KIND.into()),
            format!("{}@{}", chain_state.chain_name, chain_state.chain_version),
        );
        if let Some(raw) = self.state.get(&key).await? {
            let json = self.decrypt_state_value(&raw)?;
            let config: acteon_core::ChainConfig = serde_json::from_str(&json).map_err(|e| {
                GatewayError::ChainError(format!(
                    "failed to deserialize pinned chain definition: {e}"
                ))
            })?;
            let mut cache = self.pinned_config_cache.write();
            if cache.len() >= PINNED_CONFIG_CACHE_CAP {
                cache.clear();
            }
            cache.insert(cache_key, std::sync::Arc::new(config.clone()));
            return Ok(Some(config));
        }

        // Pre-pinning executions (no snapshot, no store entry): fall back
        // to the live registry, matching their original behavior.
        Ok(self.chains.read().get(&chain_state.chain_name).cloned())
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

        // Buffer the signal durably (FIFO) so every delivery is consumed
        // even if the chain has not reached the wait step yet — a second
        // signal must never overwrite the first. Appends happen under the
        // chain lock, so read-modify-write is safe.
        let buffer_key = signal_buffer_key(namespace, tenant, chain_id, signal_name);
        let mut buffered = self.read_signal_buffer(&buffer_key).await?;
        buffered.push(payload.clone());
        let buffer_json = serde_json::to_string(&buffered).map_err(|e| {
            GatewayError::ChainError(format!("failed to serialize signal buffer: {e}"))
        })?;
        self.state
            .set(&buffer_key, &buffer_json, Some(SIGNAL_BUFFER_TTL))
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

    /// Read the FIFO signal buffer (oldest first). Tolerates the legacy
    /// single-payload format by wrapping it in a one-element buffer.
    async fn read_signal_buffer(
        &self,
        buffer_key: &StateKey,
    ) -> Result<Vec<serde_json::Value>, GatewayError> {
        match self.state.get(buffer_key).await? {
            Some(raw) => {
                let value: serde_json::Value =
                    serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
                Ok(match value {
                    serde_json::Value::Array(items) => items,
                    single => vec![single],
                })
            }
            None => Ok(Vec::new()),
        }
    }

    /// Peek the oldest buffered signal without consuming it. The caller
    /// pops it with [`Gateway::pop_buffered_signal`] only after the chain
    /// state recording its consumption has been persisted, so a crash in
    /// between re-delivers the signal instead of losing it.
    pub(crate) async fn peek_buffered_signal(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        signal_name: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let buffer_key = signal_buffer_key(namespace, tenant, chain_id, signal_name);
        Ok(self
            .read_signal_buffer(&buffer_key)
            .await?
            .into_iter()
            .next())
    }

    /// Remove the oldest buffered signal (after its consumption has been
    /// durably recorded). Best-effort: a failure leaves the signal for
    /// at-least-once re-delivery.
    pub(crate) async fn pop_buffered_signal(
        &self,
        namespace: &str,
        tenant: &str,
        chain_id: &str,
        signal_name: &str,
    ) {
        let buffer_key = signal_buffer_key(namespace, tenant, chain_id, signal_name);
        let Ok(mut buffered) = self.read_signal_buffer(&buffer_key).await else {
            return;
        };
        if buffered.is_empty() {
            return;
        }
        buffered.remove(0);
        if buffered.is_empty() {
            let _ = self.state.delete(&buffer_key).await;
        } else if let Ok(json) = serde_json::to_string(&buffered) {
            let _ = self
                .state
                .set(&buffer_key, &json, Some(SIGNAL_BUFFER_TTL))
                .await;
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

            let chain_config = self.execution_config(&chain_state).await?.ok_or_else(|| {
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

            // The path records every visited step (linear and branched), so
            // membership is the single correct reached-check; an index
            // comparison would wrongly admit branch-skipped steps and leave
            // stale post-reset results behind.
            let reached = chain_state.execution_path.iter().any(|n| n == target_step);
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
            // Honor the target step's configured delay, as a normal
            // advancement would.
            #[allow(clippy::cast_possible_wrap)]
            let ready_at = chain_config.steps[target_idx]
                .delay_seconds
                .map_or(now.timestamp_millis(), |d| {
                    now.timestamp_millis() + (d as i64) * 1000
                });
            self.state.index_chain_ready(&pending_key, ready_at).await?;

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
