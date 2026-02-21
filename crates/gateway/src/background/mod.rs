//! Background processor for periodic tasks.
//!
//! The background processor handles:
//! - Flushing event groups when their `notify_at` time is reached
//! - Processing state machine timeouts
//! - Cleaning up expired state entries

mod workers;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, error, info};

use acteon_core::{EventGroup, StateMachineConfig};
use acteon_state::StateStore;

use acteon_crypto::PayloadEncryptor;

use crate::group_manager::GroupManager;
use crate::metrics::GatewayMetrics;

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
    /// Whether recurring action processing is enabled (default: false).
    pub enable_recurring_actions: bool,
    /// How often to check for due recurring actions (default: 60 seconds).
    pub recurring_check_interval: Duration,
    /// Whether the data retention reaper is enabled (default: false).
    pub enable_retention_reaper: bool,
    /// How often to run the data retention reaper (default: 3600 seconds).
    pub retention_check_interval: Duration,
    /// Whether periodic template sync from state store is enabled (default: false).
    pub enable_template_sync: bool,
    /// How often to sync templates from the state store (default: 30 seconds).
    pub template_sync_interval: Duration,
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
            enable_recurring_actions: false,
            recurring_check_interval: Duration::from_secs(60),
            enable_retention_reaper: false,
            retention_check_interval: Duration::from_secs(3600),
            enable_template_sync: false,
            template_sync_interval: Duration::from_secs(30),
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

/// Event emitted when a recurring action is due for dispatch.
///
/// The consumer should construct a concrete [`Action`](acteon_core::Action)
/// from the recurring action template and dispatch it through the gateway.
/// After successful dispatch, the consumer updates `last_executed_at`,
/// increments `execution_count`, and re-indexes the next occurrence.
#[derive(Debug, Clone)]
pub struct RecurringActionDueEvent {
    /// Namespace of the recurring action.
    pub namespace: String,
    /// Tenant of the recurring action.
    pub tenant: String,
    /// The recurring action ID.
    pub recurring_id: String,
    /// The deserialized recurring action definition.
    pub recurring_action: acteon_core::RecurringAction,
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
    pub record: crate::gateway::ApprovalRecord,
}

/// Background processor for periodic gateway tasks.
pub struct BackgroundProcessor {
    pub(crate) config: BackgroundConfig,
    pub(crate) group_manager: Arc<GroupManager>,
    #[allow(dead_code)] // Will be used for timeout processing
    pub(crate) state: Arc<dyn StateStore>,
    #[allow(dead_code)] // Used for timeout configuration reference
    pub(crate) state_machines: Vec<StateMachineConfig>,
    shutdown_rx: mpsc::Receiver<()>,
    /// Channel to send group flush events.
    pub(crate) group_flush_tx: Option<mpsc::Sender<GroupFlushEvent>>,
    /// Channel to send timeout events.
    pub(crate) timeout_tx: Option<mpsc::Sender<TimeoutEvent>>,
    /// Channel to send approval retry events.
    pub(crate) approval_retry_tx: Option<mpsc::Sender<ApprovalRetryEvent>>,
    /// Channel to send chain advance events.
    pub(crate) chain_advance_tx: Option<mpsc::Sender<ChainAdvanceEvent>>,
    /// Channel to send scheduled action due events.
    pub(crate) scheduled_action_tx: Option<mpsc::Sender<ScheduledActionDueEvent>>,
    /// Channel to send recurring action due events.
    pub(crate) recurring_action_tx: Option<mpsc::Sender<RecurringActionDueEvent>>,
    /// Optional payload encryptor for decrypting state values.
    pub(crate) payload_encryptor: Option<Arc<PayloadEncryptor>>,
    /// Gateway metrics for tracking background tasks.
    pub(crate) metrics: Arc<GatewayMetrics>,
    /// In-memory copy of retention policies for the reaper.
    pub(crate) retention_policies: HashMap<String, acteon_core::RetentionPolicy>,
    /// Optional gateway reference for template sync.
    pub(crate) gateway: Option<Arc<tokio::sync::RwLock<crate::gateway::Gateway>>>,
}

impl BackgroundProcessor {
    /// Create a new background processor.
    pub fn new(
        config: BackgroundConfig,
        group_manager: Arc<GroupManager>,
        state: Arc<dyn StateStore>,
        metrics: Arc<GatewayMetrics>,
        state_machines: Vec<StateMachineConfig>,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            config,
            group_manager,
            state,
            metrics,
            state_machines,
            shutdown_rx,
            group_flush_tx: None,
            timeout_tx: None,
            approval_retry_tx: None,
            chain_advance_tx: None,
            scheduled_action_tx: None,
            recurring_action_tx: None,
            payload_encryptor: None,
            retention_policies: HashMap::new(),
            gateway: None,
        }
    }

    /// Set the retention policies for the reaper.
    #[must_use]
    pub fn with_retention_policies(
        mut self,
        policies: HashMap<String, acteon_core::RetentionPolicy>,
    ) -> Self {
        self.retention_policies = policies;
        self
    }

    /// Set the gateway reference for periodic template sync.
    #[must_use]
    pub fn with_gateway(
        mut self,
        gateway: Arc<tokio::sync::RwLock<crate::gateway::Gateway>>,
    ) -> Self {
        self.gateway = Some(gateway);
        self
    }

    /// Set the payload encryptor for decrypting state values.
    #[must_use]
    pub fn with_payload_encryptor(mut self, enc: Arc<PayloadEncryptor>) -> Self {
        self.payload_encryptor = Some(enc);
        self
    }

    /// Decrypt a state value if a payload encryptor is configured, otherwise passthrough.
    fn decrypt_state_value(
        &self,
        value: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match self.payload_encryptor {
            Some(ref enc) => Ok(enc.decrypt_str(value)?),
            None => Ok(value.to_owned()),
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

    /// Set a channel to receive recurring action due events.
    #[must_use]
    pub fn with_recurring_action_channel(
        mut self,
        tx: mpsc::Sender<RecurringActionDueEvent>,
    ) -> Self {
        self.recurring_action_tx = Some(tx);
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
        let mut recurring_interval = interval(self.config.recurring_check_interval);
        let mut retention_interval = interval(self.config.retention_check_interval);
        let mut template_sync_interval = interval(self.config.template_sync_interval);

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
                _ = recurring_interval.tick(), if self.config.enable_recurring_actions => {
                    if let Err(e) = self.process_recurring_actions().await {
                        error!(error = %e, "error processing recurring actions");
                    }
                }
                _ = retention_interval.tick(), if self.config.enable_retention_reaper => {
                    if let Err(e) = self.run_retention_reaper().await {
                        error!(error = %e, "error running retention reaper");
                    }
                }
                // TODO(template-sync): Replace with reactive invalidation
                // (Redis pub/sub, PostgreSQL CDC, DynamoDB Streams).
                // See `Gateway::sync_templates_from_store` for details.
                _ = template_sync_interval.tick(), if self.config.enable_template_sync => {
                    if let Some(ref gw) = self.gateway {
                        let gw = gw.read().await;
                        match gw.sync_templates_from_store().await {
                            Ok(count) => {
                                debug!(count, "template sync completed");
                            }
                            Err(e) => {
                                error!(error = %e, "error syncing templates from store");
                            }
                        }
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
    recurring_action_tx: Option<mpsc::Sender<RecurringActionDueEvent>>,
    payload_encryptor: Option<Arc<PayloadEncryptor>>,
    metrics: Option<Arc<GatewayMetrics>>,
    gateway: Option<Arc<tokio::sync::RwLock<crate::gateway::Gateway>>>,
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
            recurring_action_tx: None,
            payload_encryptor: None,
            metrics: None,
            gateway: None,
        }
    }

    /// Set the metrics for the background processor.
    #[must_use]
    pub fn metrics(mut self, metrics: Arc<GatewayMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the payload encryptor for decrypting state values.
    #[must_use]
    pub fn payload_encryptor(mut self, enc: Arc<PayloadEncryptor>) -> Self {
        self.payload_encryptor = Some(enc);
        self
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

    /// Set the recurring action event channel.
    #[must_use]
    pub fn recurring_action_channel(mut self, tx: mpsc::Sender<RecurringActionDueEvent>) -> Self {
        self.recurring_action_tx = Some(tx);
        self
    }

    /// Set the gateway reference for periodic template sync.
    #[must_use]
    pub fn gateway(mut self, gateway: Arc<tokio::sync::RwLock<crate::gateway::Gateway>>) -> Self {
        self.gateway = Some(gateway);
        self
    }

    /// Build the background processor.
    ///
    /// Returns the processor and a shutdown sender.
    pub fn build(self) -> Result<(BackgroundProcessor, mpsc::Sender<()>), &'static str> {
        let group_manager = self.group_manager.ok_or("group_manager is required")?;
        let state = self.state.ok_or("state store is required")?;
        let metrics = self.metrics.ok_or("metrics is required")?;

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        let mut processor = BackgroundProcessor::new(
            self.config,
            group_manager,
            state,
            metrics,
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

        if let Some(tx) = self.recurring_action_tx {
            processor = processor.with_recurring_action_channel(tx);
        }

        if let Some(enc) = self.payload_encryptor {
            processor = processor.with_payload_encryptor(enc);
        }

        if let Some(gw) = self.gateway {
            processor = processor.with_gateway(gw);
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
    use acteon_state::{KeyKind, StateKey};
    use std::sync::Arc;

    use acteon_state_memory::MemoryStateStore;

    #[tokio::test]
    async fn background_processor_starts_and_stops() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
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
                enable_recurring_actions: false,
                recurring_check_interval: Duration::from_secs(60),
                enable_retention_reaper: false,
                retention_check_interval: Duration::from_secs(3600),
                enable_template_sync: false,
                template_sync_interval: Duration::from_secs(30),
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
            .metrics(Arc::new(GatewayMetrics::default()))
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
            .metrics(Arc::new(GatewayMetrics::default()))
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
                enable_recurring_actions: false,
                recurring_check_interval: Duration::from_secs(60),
                enable_retention_reaper: false,
                retention_check_interval: Duration::from_secs(3600),
                enable_template_sync: false,
                template_sync_interval: Duration::from_secs(30),
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

        // With at-least-once delivery, action data is preserved until the
        // consumer deletes it after successful dispatch. The background
        // processor only removes the pending index and timeout entry.
        let data = state.get(&sched_key).await.unwrap();
        assert!(
            data.is_some(),
            "scheduled action data should be retained for consumer cleanup (at-least-once)"
        );
        // Pending index key should be cleaned up by the processor.
        let pending_data = state.get(&pending_key).await.unwrap();
        assert!(
            pending_data.is_none(),
            "pending index should be cleaned up after dispatch"
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
            .metrics(Arc::new(GatewayMetrics::default()))
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
                enable_recurring_actions: false,
                recurring_check_interval: Duration::from_secs(60),
                enable_retention_reaper: false,
                retention_check_interval: Duration::from_secs(3600),
                enable_template_sync: false,
                template_sync_interval: Duration::from_secs(30),
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
            .metrics(Arc::new(GatewayMetrics::default()))
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
            .metrics(Arc::new(GatewayMetrics::default()))
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
                enable_recurring_actions: false,
                recurring_check_interval: Duration::from_secs(60),
                enable_retention_reaper: false,
                retention_check_interval: Duration::from_secs(3600),
                enable_template_sync: false,
                template_sync_interval: Duration::from_secs(30),
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

    // ---- Recurring action background processor tests ----

    /// Helper to build a test `RecurringAction` with sensible defaults.
    fn make_test_recurring_action(
        id: &str,
        namespace: &str,
        tenant: &str,
        cron_expr: &str,
        enabled: bool,
    ) -> acteon_core::RecurringAction {
        let now = Utc::now();
        acteon_core::RecurringAction {
            id: id.to_string(),
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            cron_expr: cron_expr.to_string(),
            timezone: "UTC".to_string(),
            enabled,
            action_template: acteon_core::RecurringActionTemplate {
                provider: "webhook".to_string(),
                action_type: "send_digest".to_string(),
                payload: serde_json::json!({"url": "https://example.com/hook"}),
                metadata: std::collections::HashMap::new(),
                dedup_key: None,
            },
            created_at: now - chrono::Duration::hours(1),
            updated_at: now,
            last_executed_at: None,
            next_execution_at: None,
            ends_at: None,
            max_executions: None,
            execution_count: 0,
            description: None,
            labels: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    async fn recurring_action_config_defaults() {
        let config = BackgroundConfig::default();
        assert!(!config.enable_recurring_actions);
        assert_eq!(config.recurring_check_interval, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn builder_with_recurring_channel() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());
        let (rec_tx, _rec_rx) = mpsc::channel(10);

        let result = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(100),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(state)
            .recurring_action_channel(rec_tx)
            .build();

        assert!(
            result.is_ok(),
            "builder with recurring channel should succeed"
        );
    }

    #[tokio::test]
    async fn dispatches_due_recurring_action() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Store an enabled recurring action.
        let recurring_id = "rec-due-001";
        let recurring =
            make_test_recurring_action(recurring_id, namespace, tenant, "*/5 * * * *", true);

        let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
        state
            .set(&rec_key, &serde_json::to_string(&recurring).unwrap(), None)
            .await
            .unwrap();

        // Index in pending_recurring with a past-due timestamp.
        let past_due = Utc::now() - chrono::Duration::seconds(10);
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        // Build processor with recurring action channel.
        let (rec_tx, mut rec_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .recurring_action_channel(rec_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Wait for the recurring action event.
        let event = tokio::time::timeout(Duration::from_secs(2), rec_rx.recv())
            .await
            .expect("should receive recurring action event within timeout");
        assert!(event.is_some(), "event should not be None");

        let event = event.unwrap();
        assert_eq!(event.recurring_id, recurring_id);
        assert_eq!(event.namespace, namespace);
        assert_eq!(event.tenant, tenant);
        assert_eq!(event.recurring_action.cron_expr, "*/5 * * * *");
        assert!(event.recurring_action.enabled);

        // The recurring action definition should still exist (not deleted after dispatch).
        let data = state.get(&rec_key).await.unwrap();
        assert!(
            data.is_some(),
            "recurring action definition should persist after dispatch"
        );

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn skips_disabled_recurring_action() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Store a disabled recurring action.
        let recurring_id = "rec-disabled-001";
        let recurring =
            make_test_recurring_action(recurring_id, namespace, tenant, "*/5 * * * *", false);

        let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
        state
            .set(&rec_key, &serde_json::to_string(&recurring).unwrap(), None)
            .await
            .unwrap();

        // Index in pending_recurring with a past-due timestamp.
        let past_due = Utc::now() - chrono::Duration::seconds(10);
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        let (rec_tx, mut rec_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .recurring_action_channel(rec_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Should NOT receive any event because action is disabled.
        let result = tokio::time::timeout(Duration::from_millis(300), rec_rx.recv()).await;
        assert!(
            result.is_err(),
            "should NOT receive event for disabled recurring action"
        );

        // Pending index should be cleaned up.
        let pending_data = state.get(&pending_key).await.unwrap();
        assert!(
            pending_data.is_none(),
            "pending index should be cleaned up for disabled action"
        );

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn retention_reaper_cleans_up_expired_data() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let metrics = Arc::new(GatewayMetrics::default());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // 1. Create a retention policy.
        let policy = acteon_core::RetentionPolicy {
            id: "ret-001".into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            enabled: true,
            audit_ttl_seconds: Some(3600),
            state_ttl_seconds: Some(3600), // 1 hour
            event_ttl_seconds: Some(3600), // 1 hour
            compliance_hold: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        let policy_key = StateKey::new("_system", "_retention", KeyKind::Retention, "ret-001");
        state
            .set(&policy_key, &serde_json::to_string(&policy).unwrap(), None)
            .await
            .unwrap();

        // 2. Create an expired chain.
        let expired_ts = (Utc::now() - chrono::Duration::seconds(7200)).to_rfc3339();
        let expired_chain = serde_json::json!({
            "id": "chain-expired",
            "status": "completed",
            "started_at": expired_ts,
            "updated_at": expired_ts,
        });
        let expired_chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, "chain-expired");
        state
            .set(
                &expired_chain_key,
                &serde_json::to_string(&expired_chain).unwrap(),
                None,
            )
            .await
            .unwrap();

        // 3. Create a fresh chain (should NOT be deleted).
        let fresh_ts = Utc::now().to_rfc3339();
        let fresh_chain = serde_json::json!({
            "id": "chain-fresh",
            "status": "completed",
            "started_at": fresh_ts,
            "updated_at": fresh_ts,
        });
        let fresh_chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, "chain-fresh");
        state
            .set(
                &fresh_chain_key,
                &serde_json::to_string(&fresh_chain).unwrap(),
                None,
            )
            .await
            .unwrap();

        // 4. Create an active chain (should NOT be deleted regardless of age).
        let active_chain = serde_json::json!({
            "id": "chain-active",
            "status": "running",
            "started_at": expired_ts,
        });
        let active_chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, "chain-active");
        state
            .set(
                &active_chain_key,
                &serde_json::to_string(&active_chain).unwrap(),
                None,
            )
            .await
            .unwrap();

        // 5. Create an expired event state.
        let expired_event = serde_json::json!({
            "id": "event-expired",
            "state": "resolved",
            "updated_at": expired_ts,
        });
        let expired_event_key =
            StateKey::new(namespace, tenant, KeyKind::EventState, "event-expired");
        state
            .set(
                &expired_event_key,
                &serde_json::to_string(&expired_event).unwrap(),
                None,
            )
            .await
            .unwrap();

        // 6. Run the reaper.
        let mut processor = BackgroundProcessor::new(
            BackgroundConfig {
                enable_retention_reaper: true,
                retention_check_interval: Duration::from_secs(3600),
                ..BackgroundConfig::default()
            },
            group_manager,
            Arc::clone(&state),
            Arc::clone(&metrics),
            Vec::new(),
            mpsc::channel(1).1,
        );

        processor.run_retention_reaper().await.unwrap();

        // 7. Verify results.
        assert!(
            state.get(&expired_chain_key).await.unwrap().is_none(),
            "expired chain should be deleted"
        );
        assert!(
            state.get(&fresh_chain_key).await.unwrap().is_some(),
            "fresh chain should remain"
        );
        assert!(
            state.get(&active_chain_key).await.unwrap().is_some(),
            "active chain should remain"
        );
        assert!(
            state.get(&expired_event_key).await.unwrap().is_none(),
            "expired event state should be deleted"
        );

        let snap = metrics.snapshot();
        assert_eq!(snap.retention_deleted_state, 2);
        assert_eq!(snap.retention_errors, 0);
        assert_eq!(snap.retention_skipped_compliance, 0);
    }

    #[tokio::test]
    async fn retention_reaper_respects_compliance_hold() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let metrics = Arc::new(GatewayMetrics::default());
        let namespace = "test-ns";
        let tenant = "compliance-tenant";

        // 1. Create a retention policy with compliance_hold = true.
        let policy = acteon_core::RetentionPolicy {
            id: "ret-compliance".into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            enabled: true,
            audit_ttl_seconds: Some(3600),
            state_ttl_seconds: Some(3600),
            event_ttl_seconds: Some(3600),
            compliance_hold: true, // <-- HOLD
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        let policy_key = StateKey::new("_system", "_retention", KeyKind::Retention, "ret-comp");
        state
            .set(&policy_key, &serde_json::to_string(&policy).unwrap(), None)
            .await
            .unwrap();

        // 2. Create an expired chain.
        let expired_ts = (Utc::now() - chrono::Duration::seconds(7200)).to_rfc3339();
        let expired_chain = serde_json::json!({
            "id": "chain-compliance",
            "status": "completed",
            "started_at": expired_ts,
        });
        let expired_chain_key =
            StateKey::new(namespace, tenant, KeyKind::Chain, "chain-compliance");
        state
            .set(
                &expired_chain_key,
                &serde_json::to_string(&expired_chain).unwrap(),
                None,
            )
            .await
            .unwrap();

        // 3. Run the reaper.
        let mut processor = BackgroundProcessor::new(
            BackgroundConfig {
                enable_retention_reaper: true,
                retention_check_interval: Duration::from_secs(3600),
                ..BackgroundConfig::default()
            },
            group_manager,
            Arc::clone(&state),
            Arc::clone(&metrics),
            Vec::new(),
            mpsc::channel(1).1,
        );

        processor.run_retention_reaper().await.unwrap();

        // 4. Verify results: chain should still be there.
        assert!(
            state.get(&expired_chain_key).await.unwrap().is_some(),
            "chain on compliance hold should NOT be deleted"
        );

        let snap = metrics.snapshot();
        assert_eq!(snap.retention_deleted_state, 0);
        assert_eq!(snap.retention_skipped_compliance, 1);
    }

    #[tokio::test]
    async fn skips_expired_recurring_action() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Store a recurring action that has already expired (ends_at in the past).
        let recurring_id = "rec-expired-001";
        let mut recurring =
            make_test_recurring_action(recurring_id, namespace, tenant, "*/5 * * * *", true);
        recurring.ends_at = Some(Utc::now() - chrono::Duration::hours(1));

        let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
        state
            .set(&rec_key, &serde_json::to_string(&recurring).unwrap(), None)
            .await
            .unwrap();

        let past_due = Utc::now() - chrono::Duration::seconds(10);
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        let (rec_tx, mut rec_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .recurring_action_channel(rec_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Should NOT receive any event because action has expired.
        let result = tokio::time::timeout(Duration::from_millis(300), rec_rx.recv()).await;
        assert!(
            result.is_err(),
            "should NOT receive event for expired recurring action"
        );

        // Pending index should be cleaned up.
        let pending_data = state.get(&pending_key).await.unwrap();
        assert!(
            pending_data.is_none(),
            "pending index should be cleaned up for expired action"
        );

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn no_channel_skips_recurring_processing() {
        let group_manager = Arc::new(GroupManager::new());
        let state = Arc::new(MemoryStateStore::new());

        // Enable recurring actions but do NOT set a channel.
        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
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
    async fn recurring_skips_recently_executed() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Store a recurring action that was executed very recently (1 second ago).
        let recurring_id = "rec-recent-001";
        let mut recurring =
            make_test_recurring_action(recurring_id, namespace, tenant, "*/5 * * * *", true);
        recurring.last_executed_at = Some(Utc::now() - chrono::Duration::seconds(1));

        let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
        state
            .set(&rec_key, &serde_json::to_string(&recurring).unwrap(), None)
            .await
            .unwrap();

        let past_due = Utc::now() - chrono::Duration::seconds(10);
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        let (rec_tx, mut rec_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .recurring_action_channel(rec_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Should NOT dispatch because last_executed_at is within 5 seconds.
        let result = tokio::time::timeout(Duration::from_millis(300), rec_rx.recv()).await;
        assert!(
            result.is_err(),
            "should NOT dispatch recently-executed recurring action"
        );

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn recurring_deleted_definition_cleans_up_pending() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        // Do NOT store a RecurringAction definition, only the pending index.
        // This simulates an orphaned pending entry (definition was deleted).
        let recurring_id = "rec-orphan-001";
        let past_due = Utc::now() - chrono::Duration::seconds(10);
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        let (rec_tx, mut rec_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .recurring_action_channel(rec_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        // Should NOT receive any event because the definition doesn't exist.
        let result = tokio::time::timeout(Duration::from_millis(300), rec_rx.recv()).await;
        assert!(
            result.is_err(),
            "should NOT dispatch when definition is missing"
        );

        // Pending index should be cleaned up (orphan removal).
        let pending_data = state.get(&pending_key).await.unwrap();
        assert!(
            pending_data.is_none(),
            "orphaned pending index should be cleaned up"
        );

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn dispatches_recurring_preserves_template_fields() {
        let group_manager = Arc::new(GroupManager::new());
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let namespace = "test-ns";
        let tenant = "test-tenant";

        let recurring_id = "rec-template-001";
        let mut recurring =
            make_test_recurring_action(recurring_id, namespace, tenant, "0 9 * * MON-FRI", true);
        recurring.action_template.provider = "email".to_string();
        recurring.action_template.action_type = "weekly_report".to_string();
        recurring.action_template.payload =
            serde_json::json!({"report_type": "weekly", "format": "pdf"});
        recurring
            .action_template
            .metadata
            .insert("team".to_string(), "engineering".to_string());
        recurring.description = Some("Weekly engineering report".to_string());
        recurring
            .labels
            .insert("env".to_string(), "production".to_string());
        recurring.execution_count = 42;

        let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
        state
            .set(&rec_key, &serde_json::to_string(&recurring).unwrap(), None)
            .await
            .unwrap();

        let past_due = Utc::now() - chrono::Duration::seconds(10);
        let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
        state
            .set(&pending_key, &past_due.timestamp_millis().to_string(), None)
            .await
            .unwrap();
        state
            .index_timeout(&pending_key, past_due.timestamp_millis())
            .await
            .unwrap();

        let (rec_tx, mut rec_rx) = mpsc::channel(10);

        let (mut processor, shutdown_tx) = BackgroundProcessorBuilder::new()
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_group_flush: false,
                enable_timeout_processing: false,
                enable_approval_retry: false,
                enable_chain_advancement: false,
                enable_scheduled_actions: false,
                enable_recurring_actions: true,
                recurring_check_interval: Duration::from_millis(50),
                ..BackgroundConfig::default()
            })
            .group_manager(group_manager)
            .state(Arc::clone(&state))
            .recurring_action_channel(rec_tx)
            .build()
            .unwrap();

        let handle = tokio::spawn(async move {
            processor.run().await;
        });

        let event = tokio::time::timeout(Duration::from_secs(2), rec_rx.recv())
            .await
            .expect("should receive event within timeout");
        let event = event.unwrap();

        // Verify all template fields are preserved through serialization roundtrip.
        assert_eq!(event.recurring_action.action_template.provider, "email");
        assert_eq!(
            event.recurring_action.action_template.action_type,
            "weekly_report"
        );
        assert_eq!(
            event.recurring_action.action_template.payload,
            serde_json::json!({"report_type": "weekly", "format": "pdf"})
        );
        assert_eq!(
            event.recurring_action.action_template.metadata.get("team"),
            Some(&"engineering".to_string())
        );
        assert_eq!(
            event.recurring_action.description.as_deref(),
            Some("Weekly engineering report")
        );
        assert_eq!(
            event.recurring_action.labels.get("env"),
            Some(&"production".to_string())
        );
        assert_eq!(event.recurring_action.execution_count, 42);

        let _ = shutdown_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }
}
