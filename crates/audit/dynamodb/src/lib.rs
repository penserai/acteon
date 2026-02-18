mod config;
mod store;
mod table;

pub use config::DynamoDbAuditConfig;
pub use store::{DynamoDbAuditStore, build_client};
pub use table::create_audit_table;
