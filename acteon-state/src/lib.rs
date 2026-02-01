pub mod error;
pub mod key;
pub mod lock;
pub mod store;
pub mod testing;

pub use error::StateError;
pub use key::{KeyKind, StateKey};
pub use lock::{DistributedLock, LockGuard};
pub use store::{CasResult, StateStore};
