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
pub mod template_engine;
mod template_management;
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
pub use watcher::RuleWatcher;
