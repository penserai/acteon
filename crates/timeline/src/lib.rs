//! Durable CDC ingestion and independently configured current-event projections.
#[cfg(feature = "archive")]
pub mod archive;
#[cfg(feature = "bus")]
pub mod bus;
pub mod clickhouse;
pub mod config;
pub mod debezium;
pub mod elasticsearch;
pub mod ledger;
pub mod memory;
pub mod metrics;
mod predicate;
pub mod search;
pub mod service;
pub mod store;
pub use acteon_core::timeline::*;
pub use service::TimelineService;
pub use store::TimelineStore;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid timeline input: {0}")]
    Invalid(String),
    #[error("timeline revision conflict")]
    Conflict,
    #[error("timeline capacity exhausted")]
    Capacity,
    #[error("timeline storage: {0}")]
    Storage(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    State(#[from] acteon_state::StateError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn digest(value: &impl serde::Serialize) -> String {
    use sha2::{Digest, Sha256};
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> = map
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonical).collect())
            }
            value => value,
        }
    }
    let canonical = canonical(serde_json::to_value(value).expect("serializable timeline value"));
    hex::encode(Sha256::digest(
        serde_json::to_vec(&canonical).expect("canonical JSON"),
    ))
}
