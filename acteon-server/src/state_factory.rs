use std::sync::Arc;

use acteon_state::{DistributedLock, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

use crate::config::StateConfig;
use crate::error::ServerError;

/// A state store and distributed lock pair.
pub type StatePair = (Arc<dyn StateStore>, Arc<dyn DistributedLock>);

/// Construct a `StateStore` and `DistributedLock` pair from configuration.
///
/// Currently only the `"memory"` backend is supported. Extend this function
/// to add Redis or other backends in the future.
pub fn create_state(config: &StateConfig) -> Result<StatePair, ServerError> {
    match config.backend.as_str() {
        "memory" => {
            let store = Arc::new(MemoryStateStore::new());
            let lock = Arc::new(MemoryDistributedLock::new());
            Ok((store, lock))
        }
        other => Err(ServerError::Config(format!(
            "unsupported state backend: {other}"
        ))),
    }
}
