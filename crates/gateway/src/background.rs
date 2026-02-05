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

    /// Run the background processor until shutdown is signaled.
    pub async fn run(&mut self) {
        info!("background processor starting");

        let mut group_interval = interval(self.config.group_flush_interval);
        let mut timeout_interval = interval(self.config.timeout_check_interval);
        let mut cleanup_interval = interval(self.config.cleanup_interval);

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
