pub mod background;
pub mod builder;
pub mod error;
pub mod gateway;
pub mod group_manager;
pub mod metrics;
pub mod watcher;

pub use acteon_executor::{DeadLetterEntry, DeadLetterQueue, DeadLetterSink};
pub use background::{
    ApprovalRetryEvent, BackgroundConfig, BackgroundProcessor, BackgroundProcessorBuilder,
    GroupFlushEvent, TimeoutEvent,
};
pub use builder::GatewayBuilder;
pub use error::GatewayError;
pub use gateway::{ApprovalRecord, ApprovalStatus, Gateway};
pub use group_manager::GroupManager;
pub use metrics::{GatewayMetrics, MetricsSnapshot};
pub use watcher::RuleWatcher;
