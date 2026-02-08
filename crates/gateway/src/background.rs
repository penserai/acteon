//! Background processor for periodic tasks.
//!
//! The background processor handles:
//! - Flushing event groups when their `notify_at` time is reached
//! - Processing state machine timeouts
//! - Cleaning up expired state entries

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use acteon_core::{EventGroup, StateMachineConfig};
use acteon_state::{KeyKind, StateKey, StateStore};

use crate::gateway::ApprovalRecord;
use crate::group_manager::GroupManager;

/// Configuration for the background processor.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackgroundConfig {
    /// How often to check for ready groups (default: 5 seconds).
    pub group_flush_interval: Duration,
    /// How often to check for state machine timeouts (default: 10 seconds).
    pub timeout_check_interval: Duration,
    /// How often to run cleanup tasks (default: 60 seconds).
    pub cleanup_interval: Duration,
    /// Whether group flushing is enabled.
    pub enable_group_flush: bool,
    /// Whether timeout processing is enabled.
    pub enable_timeout_processing: bool,
    /// Whether approval notification retry is enabled (default: true).
    pub enable_approval_retry: bool,
    /// Whether chain advancement is enabled (default: true).
    pub enable_chain_advancement: bool,
    /// How often to check for pending chains (default: 5 seconds).
    pub chain_check_interval: Duration,
    /// Whether scheduled action processing is enabled (default: false).
    pub enable_scheduled_actions: bool,
    /// How often to check for due scheduled actions (default: 5 seconds).
    pub scheduled_check_interval: Duration,
    /// Namespace to scan for timeouts (required for timeout processing).
    pub namespace: String,
    /// Tenant to scan for timeouts (required for timeout processing).
    pub tenant: String,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            group_flush_interval: Duration::from_secs(5),
            timeout_check_interval: Duration::from_secs(10),
            cleanup_interval: Duration::from_secs(60),
            enable_group_flush: true,
            enable_timeout_processing: true,
            enable_approval_retry: true,
            enable_chain_advancement: true,
            chain_check_interval: Duration::from_secs(5),
            enable_scheduled_actions: false,
            scheduled_check_interval: Duration::from_secs(5),
            namespace: String::new(),
            tenant: String::new(),
        }
    }
}

/// Event emitted when a group is flushed.
#[derive(Debug, Clone)]
pub struct GroupFlushEvent {
    /// The flushed group.
    pub group: EventGroup,
    /// When the flush occurred.
    pub flushed_at: chrono::DateTime<Utc>,
}

/// Event emitted when a state machine timeout fires.
#[derive(Debug, Clone)]
pub struct TimeoutEvent {
    /// The fingerprint of the event that timed out.
    pub fingerprint: String,
    /// The state machine name.
    pub state_machine: String,
    /// The previous state before timeout.
    pub previous_state: String,
    /// The new state after timeout transition.
    pub new_state: String,
    /// When the timeout fired.
    pub fired_at: chrono::DateTime<Utc>,
    /// Captured trace context from the event that created the timeout.
    pub trace_context: std::collections::HashMap<String, String>,
}

/// Event emitted when a chain needs advancement.
#[derive(Debug, Clone)]
pub struct ChainAdvanceEvent {
    /// Namespace of the chain.
    pub namespace: String,
    /// Tenant of the chain.
    pub tenant: String,
    /// The chain execution ID.
    pub chain_id: String,
}

/// Event emitted when a scheduled action is due for dispatch.
///
// TODO(scheduled-actions): The consumer of this event must be careful not to
// re-dispatch the action through the full rule pipeline, as the same Schedule
// rule could fire again creating an infinite loop. Either bypass rules entirely
// or mark the action to prevent re-scheduling.
#[derive(Debug, Clone)]
pub struct ScheduledActionDueEvent {
    /// Namespace of the scheduled action.
    pub namespace: String,
    /// Tenant of the scheduled action.
    pub tenant: String,
    /// The scheduled action ID.
    pub action_id: String,
    /// The serialized action to dispatch.
    pub action: acteon_core::Action,
}

/// Event emitted when a pending approval needs notification retry.
#[derive(Debug, Clone)]
pub struct ApprovalRetryEvent {
    /// Namespace of the approval.
    pub namespace: String,
    /// Tenant of the approval.
    pub tenant: String,
    /// The approval ID.
    pub approval_id: String,
    /// The full approval record (contains action, URLs, etc.).
    pub record: ApprovalRecord,
}

/// Background processor for periodic gateway tasks.
pub struct BackgroundProcessor {
    config: BackgroundConfig,
    group_manager: Arc<GroupManager>,
    #[allow(dead_code)] // Will be used for timeout processing
    state: Arc<dyn StateStore>,
    #[allow(dead_code)] // Used for timeout configuration reference
    state_machines: Vec<StateMachineConfig>,
    shutdown_rx: mpsc::Receiver<()>,
    /// Channel to send group flush events.
    group_flush_tx: Option<mpsc::Sender<GroupFlushEvent>>,
    /// Channel to send timeout events.
    timeout_tx: Option<mpsc::Sender<TimeoutEvent>>,
    /// Channel to send approval retry events.
    approval_retry_tx: Option<mpsc::Sender<ApprovalRetryEvent>>,
    /// Channel to send chain advance events.
    chain_advance_tx: Option<mpsc::Sender<ChainAdvanceEvent>>,
    /// Channel to send scheduled action due events.
    scheduled_action_tx: Option<mpsc::Sender<ScheduledActionDueEvent>>,
}

impl BackgroundProcessor {
    /// Create a new background processor.
    pub fn new(
        config: BackgroundConfig,
        group_manager: Arc<GroupManager>,
        state: Arc<dyn StateStore>,
        state_machines: Vec<StateMachineConfig>,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            config,
            group_manager,
            state,
            state_machines,
            shutdown_rx,
            group_flush_tx: None,
            timeout_tx: None,
            approval_retry_tx: None,
            chain_advance_tx: None,
            scheduled_action_tx: None,
        }
    }

    /// Set a channel to receive group flush events.
    #[must_use]
    pub fn with_group_flush_channel(mut self, tx: mpsc::Sender<GroupFlushEvent>) -> Self {
        self.group_flush_tx = Some(tx);
        self
    }

    /// Set a channel to receive timeout events.
    #[must_use]
    pub fn with_timeout_channel(mut self, tx: mpsc::Sender<TimeoutEvent>) -> Self {
        self.timeout_tx = Some(tx);
        self
    }

    /// Set a channel to receive approval retry events.
    #[must_use]
    pub fn with_approval_retry_channel(mut self, tx: mpsc::Sender<ApprovalRetryEvent>) -> Self {
        self.approval_retry_tx = Some(tx);
        self
    }

    /// Set a channel to receive chain advance events.
    #[must_use]
    pub fn with_chain_advance_channel(mut self, tx: mpsc::Sender<ChainAdvanceEvent>) -> Self {
        self.chain_advance_tx = Some(tx);
        self
    }

    /// Set a channel to receive scheduled action due events.
    #[must_use]
    pub fn with_scheduled_action_channel(
        mut self,
        tx: mpsc::Sender<ScheduledActionDueEvent>,
    ) -> Self {
        self.scheduled_action_tx = Some(tx);
        self
    }

    /// Run the background processor until shutdown is signaled.
    pub async fn run(&mut self) {
        info!("background processor starting");

        let mut group_interval = interval(self.config.group_flush_interval);
        let mut timeout_interval = interval(self.config.timeout_check_interval);
        let mut cleanup_interval = interval(self.config.cleanup_interval);
        let mut chain_interval = interval(self.config.chain_check_interval);
        let mut scheduled_interval = interval(self.config.scheduled_check_interval);

        loop {
            tokio::select! {
                _ = self.shutdown_rx.recv() => {
                    info!("background processor received shutdown signal");
                    break;
                }
                _ = group_interval.tick(), if self.config.enable_group_flush => {
                    if let Err(e) = self.flush_ready_groups().await {
                        error!(error = %e, "error flushing groups");
                    }
                }
                _ = timeout_interval.tick(), if self.config.enable_timeout_processing => {
                    if let Err(e) = self.process_timeouts().await {
                        error!(error = %e, "error processing timeouts");
                    }
                }
                _ = chain_interval.tick(), if self.config.enable_chain_advancement => {
                    if let Err(e) = self.advance_pending_chains().await {
                        error!(error = %e, "error advancing chains");
                    }
                }
                _ = scheduled_interval.tick(), if self.config.enable_scheduled_actions => {
                    if let Err(e) = self.process_scheduled_actions().await {
                        error!(error = %e, "error processing scheduled actions");
                    }
                }
                _ = cleanup_interval.tick() => {
                    if let Err(e) = self.run_cleanup().await {
                        error!(error = %e, "error running cleanup");
                    }
                }
            }
        }

        info!("background processor stopped");
    }

    /// Flush all groups that are ready.
    async fn flush_ready_groups(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ready_groups = self.group_manager.get_ready_groups();

        if ready_groups.is_empty() {
            return Ok(());
        }

        debug!(count = ready_groups.len(), "flushing ready groups");

        for group in ready_groups {
            let group_key = group.group_key.clone();

            // Flush the group (marks it as notified)
            if let Some(flushed_group) = self.group_manager.flush_group(&group_key) {
                let flushed_at = Utc::now();

                // Remove from pending groups index
                // Note: We need namespace/tenant from the group labels or stored metadata
                // For now, we'll just clean up the in-memory state

                info!(
                    group_id = %flushed_group.group_id,
                    group_key = %group_key,
                    event_count = flushed_group.size(),
                    "group flushed"
                );

                // Send flush event if channel is configured
                if let Some(ref tx) = self.group_flush_tx {
                    let event = GroupFlushEvent {
                        group: flushed_group.clone(),
                        flushed_at,
                    };
                    if tx.send(event).await.is_err() {
                        warn!("group flush event channel closed");
                    }
                }

                // Remove the group from memory after processing
                self.group_manager.remove_group(&group_key);
            }
        }

        Ok(())
    }

    /// Process state machine timeouts.
    ///
    /// Uses an indexed approach to efficiently find expired timeouts in O(log N + M)
    /// where M is the number of expired entries, instead of scanning all timeout keys.
    async fn process_timeouts(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let now = Utc::now();
        let now_ms = now.timestamp_millis();

        // Get only the expired timeout keys using the efficient index query.
        let expired_keys = self.state.get_expired_timeouts(now_ms).await?;

        if expired_keys.is_empty() {
            return Ok(());
        }

        debug!(count = expired_keys.len(), "processing expired timeouts");

        for canonical_key in expired_keys {
            // Parse namespace and tenant from the key (format: namespace:tenant:kind:id)
            let key_parts: Vec<&str> = canonical_key.splitn(4, ':').collect();
            let (namespace, tenant, fingerprint) = if key_parts.len() >= 4 {
                (
                    key_parts[0].to_string(),
                    key_parts[1].to_string(),
                    key_parts[3].to_string(),
                )
            } else {
                warn!(key = %canonical_key, "invalid timeout key format");
                continue;
            };

            // Fetch the timeout data from the state store
            let timeout_key = StateKey::new(
                namespace.as_str(),
                tenant.as_str(),
                KeyKind::EventTimeout,
                &fingerprint,
            );

            let Some(value) = self.state.get(&timeout_key).await? else {
                // Timeout was already processed or deleted, remove from index
                self.state.remove_timeout_index(&timeout_key).await?;
                continue;
            };

            // Parse the timeout entry
            let Ok(timeout_data) = serde_json::from_str::<serde_json::Value>(&value) else {
                warn!(key = %canonical_key, "failed to parse timeout data");
                continue;
            };

            // fingerprint is already parsed from the key above
            let state_machine_name = timeout_data
                .get("state_machine")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let current_state = timeout_data
                .get("current_state")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let transition_to = timeout_data
                .get("transition_to")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let trace_context: std::collections::HashMap<String, String> = timeout_data
                .get("trace_context")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            info!(
                fingerprint = %fingerprint,
                namespace = %namespace,
                tenant = %tenant,
                state_machine = %state_machine_name,
                from_state = %current_state,
                to_state = %transition_to,
                "processing expired timeout"
            );

            // Update the event state
            let state_key = StateKey::new(
                namespace.as_str(),
                tenant.as_str(),
                KeyKind::EventState,
                &fingerprint,
            );

            let new_state_value = serde_json::json!({
                "state": &transition_to,
                "fingerprint": &fingerprint,
                "updated_at": now.to_rfc3339(),
                "transitioned_by": "timeout",
            });

            self.state
                .set(&state_key, &new_state_value.to_string(), None)
                .await?;

            // Delete the processed timeout entry and remove from index
            self.state.delete(&timeout_key).await?;
            self.state.remove_timeout_index(&timeout_key).await?;

            // Send timeout event if channel is configured
            if let Some(ref tx) = self.timeout_tx {
                let event = TimeoutEvent {
                    fingerprint,
                    state_machine: state_machine_name,
                    previous_state: current_state,
                    new_state: transition_to,
                    fired_at: now,
                    trace_context,
                };
                if tx.send(event).await.is_err() {
                    warn!("timeout event channel closed");
                }
            }
        }

        Ok(())
    }

    /// Run periodic cleanup tasks, including approval notification retry sweep.
    async fn run_cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Clean up resolved/notified groups that are no longer needed
        let groups = self.group_manager.list_pending_groups();
        debug!(pending_groups = groups.len(), "cleanup: checking groups");

        // Sweep for pending approvals that need notification retry.
        if self.config.enable_approval_retry
            && let Some(ref tx) = self.approval_retry_tx
        {
            self.sweep_approval_retries(tx).await;
        }

        Ok(())
    }

    /// Scan for pending approvals with `notification_sent == false` and emit retry events.
    async fn sweep_approval_retries(&self, tx: &mpsc::Sender<ApprovalRetryEvent>) {
        let entries = match self.state.scan_keys_by_kind(KeyKind::Approval).await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(error = %e, "failed to scan approval keys for retry sweep");
                return;
            }
        };

        let now = Utc::now();
        let mut retry_count = 0u32;

        for (key, value) in entries {
            // Skip claim keys (format: namespace:tenant:approval:id:claim)
            if key.ends_with(":claim") {
                continue;
            }

            let record: ApprovalRecord = match serde_json::from_str(&value) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Only retry pending, unsent, non-expired approvals
            if record.status != "pending" || record.notification_sent || record.expires_at <= now {
                continue;
            }

            // Parse namespace and tenant from key (format: namespace:tenant:approval:id)
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                continue;
            }
            let namespace = parts[0].to_string();
            let tenant = parts[1].to_string();

            let event = ApprovalRetryEvent {
                namespace,
                tenant,
                approval_id: record.token.clone(),
                record,
            };

            if tx.send(event).await.is_err() {
                warn!("approval retry event channel closed");
                return;
            }
            retry_count += 1;
        }

        if retry_count > 0 {
            debug!(count = retry_count, "emitted approval retry events");
        }
    }

    /// Process scheduled actions that are due for dispatch.
    ///
    /// Uses the timeout index for efficient O(log N + M) lookups of expired
    /// `PendingScheduled` keys, loads the corresponding `ScheduledAction` data,
    /// and emits dispatch events.
    ///
    /// Uses an atomic claim key (`check_and_set`) to prevent double-dispatch
    /// when multiple server instances poll concurrently.
    async fn process_scheduled_actions(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref tx) = self.scheduled_action_tx else {
            return Ok(());
        };

        let now_ms = Utc::now().timestamp_millis();

        // Use the timeout index for efficient O(log N + M) queries instead of
        // scanning all PendingScheduled keys (which would be O(N)).
        let expired_keys = self.state.get_expired_timeouts(now_ms).await?;

        // Filter to only PendingScheduled keys (the timeout index is shared
        // with EventTimeout keys).
        let due_keys: Vec<String> = expired_keys
            .into_iter()
            .filter(|k| k.contains(":pending_scheduled:"))
            .collect();

        if due_keys.is_empty() {
            return Ok(());
        }

        let mut dispatched = 0u32;

        for key in due_keys {
            // Parse namespace:tenant:pending_scheduled:action_id
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                warn!(key = %key, "invalid pending scheduled key format");
                continue;
            }
            let namespace = parts[0];
            let tenant = parts[1];
            let action_id = parts[3];

            // Atomically claim this scheduled action to prevent double-dispatch.
            // If another instance already claimed it, `check_and_set` returns false.
            let claim_key = StateKey::new(
                namespace,
                tenant,
                KeyKind::ScheduledAction,
                format!("{action_id}:claim"),
            );
            let claimed = self
                .state
                .check_and_set(
                    &claim_key,
                    "claimed",
                    Some(std::time::Duration::from_secs(60)),
                )
                .await?;
            if !claimed {
                debug!(action_id = %action_id, "scheduled action already claimed by another instance");
                continue;
            }

            // Load the scheduled action data.
            let sched_key = StateKey::new(namespace, tenant, KeyKind::ScheduledAction, action_id);
            let Some(data_str) = self.state.get(&sched_key).await? else {
                // Already processed, clean up pending key.
                let pending_key =
                    StateKey::new(namespace, tenant, KeyKind::PendingScheduled, action_id);
                self.state.delete(&pending_key).await?;
                self.state.remove_timeout_index(&pending_key).await?;
                continue;
            };

            let Ok(data) = serde_json::from_str::<serde_json::Value>(&data_str) else {
                warn!(action_id = %action_id, "failed to parse scheduled action data");
                continue;
            };

            let Some(action_val) = data.get("action") else {
                warn!(action_id = %action_id, "scheduled action missing action field");
                continue;
            };

            let Ok(action) = serde_json::from_value::<acteon_core::Action>(action_val.clone())
            else {
                warn!(action_id = %action_id, "failed to deserialize scheduled action");
                continue;
            };

            // Delete the scheduled action data and pending key.
            self.state.delete(&sched_key).await?;
            let pending_key =
                StateKey::new(namespace, tenant, KeyKind::PendingScheduled, action_id);
            self.state.delete(&pending_key).await?;
            self.state.remove_timeout_index(&pending_key).await?;

            info!(
                action_id = %action_id,
                namespace = %namespace,
                tenant = %tenant,
                "dispatching scheduled action"
            );

            let event = ScheduledActionDueEvent {
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                action_id: action_id.to_string(),
                action,
            };

            if tx.send(event).await.is_err() {
                warn!("scheduled action event channel closed");
                return Ok(());
            }
            dispatched += 1;
        }

        if dispatched > 0 {
            debug!(count = dispatched, "dispatched due scheduled actions");
        }

        Ok(())
    }

    /// Scan for pending chains and emit advance events.
    ///
    /// Uses the chain-ready index for efficient O(log N + M) lookups instead
    /// of scanning all pending chain keys.
    async fn advance_pending_chains(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref tx) = self.chain_advance_tx else {
            return Ok(());
        };

        let now_ms = Utc::now().timestamp_millis();
        let ready_keys = self.state.get_ready_chains(now_ms).await?;

        if ready_keys.is_empty() {
            return Ok(());
        }

        debug!(count = ready_keys.len(), "checking ready chains");

        for key in ready_keys {
            // Parse namespace:tenant:pending_chains:chain_id from the canonical key.
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                warn!(key = %key, "invalid chain ready key format");
                continue;
            }

            let event = ChainAdvanceEvent {
                namespace: parts[0].to_string(),
                tenant: parts[1].to_string(),
                chain_id: parts[3].to_string(),
            };

            if tx.send(event).await.is_err() {
                warn!("chain advance event channel closed");
                return Ok(());
            }
        }

        Ok(())
    }
}

/// Builder for creating a background processor.
pub struct BackgroundProcessorBuilder {
    config: BackgroundConfig,
    group_manager: Option<Arc<GroupManager>>,
    state: Option<Arc<dyn StateStore>>,
    state_machines: Vec<StateMachineConfig>,
    group_flush_tx: Option<mpsc::Sender<GroupFlushEvent>>,
    timeout_tx: Option<mpsc::Sender<TimeoutEvent>>,
    approval_retry_tx: Option<mpsc::Sender<ApprovalRetryEvent>>,
    chain_advance_tx: Option<mpsc::Sender<ChainAdvanceEvent>>,
    scheduled_action_tx: Option<mpsc::Sender<ScheduledActionDueEvent>>,
}

impl BackgroundProcessorBuilder {
    /// Create a new builder with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: BackgroundConfig::default(),
            group_manager: None,
            state: None,
            state_machines: Vec::new(),
            group_flush_tx: None,
            timeout_tx: None,
            approval_retry_tx: None,
            chain_advance_tx: None,
            scheduled_action_tx: None,
        }
    }

    /// Set the configuration.
    #[must_use]
    pub fn config(mut self, config: BackgroundConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the group manager.
    #[must_use]
    pub fn group_manager(mut self, manager: Arc<GroupManager>) -> Self {
        self.group_manager = Some(manager);
        self
    }

    /// Set the state store.
    #[must_use]
    pub fn state(mut self, state: Arc<dyn StateStore>) -> Self {
        self.state = Some(state);
        self
    }

    /// Add state machine configurations.
    #[must_use]
    pub fn state_machines(mut self, machines: Vec<StateMachineConfig>) -> Self {
        self.state_machines = machines;
        self
    }

    /// Set the group flush event channel.
    #[must_use]
    pub fn group_flush_channel(mut self, tx: mpsc::Sender<GroupFlushEvent>) -> Self {
        self.group_flush_tx = Some(tx);
        self
    }

    /// Set the timeout event channel.
    #[must_use]
    pub fn timeout_channel(mut self, tx: mpsc::Sender<TimeoutEvent>) -> Self {
        self.timeout_tx = Some(tx);
        self
    }

    /// Set the approval retry event channel.
    #[must_use]
    pub fn approval_retry_channel(mut self, tx: mpsc::Sender<ApprovalRetryEvent>) -> Self {
        self.approval_retry_tx = Some(tx);
        self
    }

    /// Set the chain advance event channel.
    #[must_use]
    pub fn chain_advance_channel(mut self, tx: mpsc::Sender<ChainAdvanceEvent>) -> Self {
        self.chain_advance_tx = Some(tx);
        self
    }

    /// Set the scheduled action event channel.
    #[must_use]
    pub fn scheduled_action_channel(mut self, tx: mpsc::Sender<ScheduledActionDueEvent>) -> Self {
        self.scheduled_action_tx = Some(tx);
        self
    }

    /// Build the background processor.
    ///
    /// Returns the processor and a shutdown sender.
    pub fn build(self) -> Result<(BackgroundProcessor, mpsc::Sender<()>), &'static str> {
        let group_manager = self.group_manager.ok_or("group_manager is required")?;
        let state = self.state.ok_or("state store is required")?;

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        let mut processor = BackgroundProcessor::new(
            self.config,
            group_manager,
            state,
            self.state_machines,
            shutdown_rx,
        );

        if let Some(tx) = self.group_flush_tx {
            processor = processor.with_group_flush_channel(tx);
        }

        if let Some(tx) = self.timeout_tx {
            processor = processor.with_timeout_channel(tx);
        }

        if let Some(tx) = self.approval_retry_tx {
            processor = processor.with_approval_retry_channel(tx);
        }

        if let Some(tx) = self.chain_advance_tx {
            processor = processor.with_chain_advance_channel(tx);
        }

        if let Some(tx) = self.scheduled_action_tx {
            processor = processor.with_scheduled_action_channel(tx);
        }

        Ok((processor, shutdown_tx))
    }
}

impl Default for BackgroundProcessorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_state_memory::MemoryStateStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn background_processor_starts_and_stops() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .config(BackgroundConfig {
                group_flush_interval: Duration::from_millis(100),
                timeout_check_interval: Duration::from_millis(100),
                cleanup_interval: Duration::from_millis(100),
                enable_group_flush: true,
                enable_timeout_processing: true,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                chain_check_interval: Duration::from_secs(5),
                enable_scheduled_actions: false,
                scheduled_check_interval: Duration::from_secs(5),
                namespace: "test".to_string(),
                tenant: "test-tenant".to_string(),
            })
            .group_manager(group_manager)
            .state(state)
            .build()
            .unwrap();

        // Start processor in background
        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Let it run briefly
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Signal shutdown
        let _ = shutdown_tx.send(()).await;

        // Wait for processor to stop
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "processor should stop within timeout");
    }

    #[tokio::test]
    async fn background_processor_scheduled_action_config_defaults() {
        let config = BackgroundConfig::default();
        assert!(!config.enable_scheduled_actions);
        assert_eq!(config.scheduled_check_interval, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn background_processor_builder_with_scheduled_channel() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());
        let (sched_tx, _sched_rx) = mpsc::channel(10);

        let result = BackgroundProcessorBuilder::new()
            .config(BackgroundConfig {
                enable_scheduled_actions: true,
                scheduled_check_interval: Duration::from_millis(100),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(state)
            .scheduled_action_channel(sched_tx)
            .build();

        assert!(
            result.is_ok(),
            "builder with scheduled channel should succeed"
        );
    }

    #[tokio::test]
    async fn background_processor_dispatches_due_scheduled_action() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Manually store a scheduled action that is already due.
        let action_id = "sched-due-001";
        let action = acteon_core::Action::new(
            namespace,
            tenant,
            "email",
            "send_email",
            serde_json::json!({"to": "user@test.com"}),
        );
        let now = Utc::now();
        let past_due = now - chrono::Duration::seconds(10);

        // Store the scheduled action data.
        let sched_key = StateKey::new(namespace, tenant, KeyKind::ScheduledAction, action_id);
        let sched_data = serde_json::json!({
            "action_id": action_id,
            "action": action,
            "scheduled_for": past_due.to_rfc3339(),
            "created_at": (past_due - chrono::Duration::seconds(60)).to_rfc3339(),
        });
        state
            .set(&sched_key, &sched_data.to_string(), None)
            .await
            .unwrap();

        // Store in the pending index with a past-due timestamp.
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingScheduled, action_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        // Build processor with scheduled action channel.
        let (sched_tx, mut sched_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .config(BackgroundConfig {
                group_flush_interval: Duration::from_secs(100),
                timeout_check_interval: Duration::from_secs(100),
                cleanup_interval: Duration::from_secs(100),
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                chain_check_interval: Duration::from_secs(100),
                enable_scheduled_actions: true,
                scheduled_check_interval: Duration::from_millis(50),
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .scheduled_action_channel(sched_tx)
            .build()
            .unwrap();

        // Start processor.
        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Wait for the scheduled action event.
        let event = tokio::time::timeout(Duration::from_secs(2), sched_rx.recv())
            .await
            .expect("should receive scheduled action event within timeout");
        assert!(event.is_some(), "event should not be None");

        let event = event.unwrap();
        assert_eq!(event.action_id, action_id);
        assert_eq!(event.namespace, namespace);
        assert_eq!(event.tenant, tenant);
        assert_eq!(event.action.action_type, "send_email");

        // Verify cleanup: scheduled action data should be deleted.
        let data = state.get(&sched_key).await.unwrap();
        assert!(
            data.is_none(),
            "scheduled action data should be cleaned up after dispatch"
        );

        // Shutdown.
        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn background_processor_skips_not_yet_due_action() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Store a scheduled action that is NOT yet due (1 hour in the future).
        let action_id = "sched-future-001";
        let action = acteon_core::Action::new(
            namespace,
            tenant,
            "email",
            "send_email",
            serde_json::json!({"to": "user@test.com"}),
        );
        let future_time = Utc::now() + chrono::Duration::hours(1);

        let sched_key = StateKey::new(namespace, tenant, KeyKind::ScheduledAction, action_id);
        let sched_data = serde_json::json!({
            "action_id": action_id,
            "action": action,
            "scheduled_for": future_time.to_rfc3339(),
            "created_at": Utc::now().to_rfc3339(),
        });
        state
            .set(&sched_key, &sched_data.to_string(), None)
            .await
            .unwrap();

        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingScheduled, action_id);
        state
            .set(
                &pending_key,
                &future_time.timestamp_millis().to_string(),
                None,
            )
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, future_time.timestamp_millis())
            .await
            .unwrap();

        let (sched_tx, mut sched_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .config(BackgroundConfig {
                group_flush_interval: Duration::from_secs(100),
                timeout_check_interval: Duration::from_secs(100),
                cleanup_interval: Duration::from_secs(100),
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                chain_check_interval: Duration::from_secs(100),
                enable_scheduled_actions: true,
                scheduled_check_interval: Duration::from_millis(50),
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .scheduled_action_channel(sched_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Wait a bit and verify no event was received (action is not yet due).
        let result = tokio::time::timeout(Duration::from_millis(300), sched_rx.recv()).await;
        assert!(
            result.is_err(),
            "should NOT receive event for future-scheduled action"
        );

        // Data should still exist in state store.
        let data = state.get(&sched_key).await.unwrap();
        assert!(data.is_some(), "future-scheduled action data should remain");

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn background_processor_no_channel_skips_scheduled() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());

        // Enable scheduled actions but do NOT set a channel.
        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                chain_check_interval: Duration::from_secs(100),
                enable_scheduled_actions: true,
                scheduled_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(state)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Should not panic or error even without a channel.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let _ = shutdown_tx.send(()).await;
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "processor should stop cleanly");
    }

    #[tokio::test]
    async fn background_processor_flushes_ready_groups() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());

        // Add a group that's already ready (notify_at in the past)
        let past = Utc::now() - chrono::Duration::seconds(10);
        let mut group = EventGroup::new("group-1", "key-1", past);
        group.add_event(acteon_core::GroupedEvent::new(
            acteon_core::types::ActionId::new("action-1".to_string()),
            serde_json::json!({"test": true}),
        ));
        group_manager
            .groups
            .write()
            .insert("key-1".to_string(), group);

        // Create channel to receive flush events
        let (flush_tx, mut flush_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .config(BackgroundConfig {
                group_flush_interval: Duration::from_millis(50),
                timeout_check_interval: Duration::from_secs(100),
                cleanup_interval: Duration::from_secs(100),
                enable_group_flush: true,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                chain_check_interval: Duration::from_secs(5),
                enable_scheduled_actions: false,
                scheduled_check_interval: Duration::from_secs(5),
                namespace: "test".to_string(),
                tenant: "test-tenant".to_string(),
            })
            .group_manager(Arc::clone(&group_manager))
            .state(state)
            .group_flush_channel(flush_tx)
            .build()
            .unwrap();

        // Start processor
        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Wait for flush event
        let event = tokio::time::timeout(Duration::from_secs(1), flush_rx.recv()).await;
        assert!(event.is_ok(), "should receive flush event");
        let event = event.unwrap();
        assert!(event.is_some(), "flush event should not be None");

        let flush_event = event.unwrap();
        assert_eq!(flush_event.group.group_id, "group-1");
        assert_eq!(flush_event.group.size(), 1);

        // Shutdown
        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;

        // Group should be removed after flush
        assert_eq!(group_manager.active_group_count(), 0);
    }
}
