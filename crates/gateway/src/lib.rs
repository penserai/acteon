pub(crate) mod audit_helpers;
pub mod background;
pub mod builder;
pub mod chain;
pub mod circuit_breaker;
pub mod encrypting_dlq;
pub mod enrichment;
pub mod error;
pub mod gateway;
pub mod group_manager;
pub mod metrics;
mod quota_enforcement;
mod silence_enforcement;
pub(crate) mod sync_state;
pub mod task_chain_bridge;
pub mod task_engine;
pub mod template_engine;
mod template_management;
mod time_interval_management;
pub mod watcher;

pub use acteon_executor::{DeadLetterEntry, DeadLetterQueue, DeadLetterSink};
pub use background::{
    ApprovalRetryEvent, BackgroundConfig, BackgroundProcessor, BackgroundProcessorBuilder,
    ChainAdvanceEvent, GroupFlushEvent, TimeoutEvent,
};
pub use builder::GatewayBuilder;
pub use circuit_breaker::{CircuitBreakerConfig, CircuitBreakerRegistry, CircuitState};
pub use encrypting_dlq::EncryptingDeadLetterSink;
pub use error::GatewayError;
pub use gateway::{ApprovalKey, ApprovalKeySet, ApprovalRecord, ApprovalStatus, Gateway};
pub use group_manager::GroupManager;
pub use metrics::{GatewayMetrics, MetricsSnapshot, ProviderMetrics, ProviderStatsSnapshot};
pub use silence_enforcement::CachedSilence;
pub use task_chain_bridge::{
    BridgeError as TaskChainBridgeError, link_task_to_chain, project_chain_status_to_task_state,
    project_chain_to_linked_task,
};
pub use task_engine::{
    MAX_CAS_RETRY_ATTEMPTS as A2A_MAX_CAS_RETRY_ATTEMPTS,
    MESSAGE_DEDUP_TTL as A2A_MESSAGE_DEDUP_TTL, ScopedTaskEngine, TaskEngine, TaskEngineError,
    TaskScope,
};
pub use time_interval_management::{TimeIntervalDecision, time_interval_cache_id};
pub use watcher::RuleWatcher;
