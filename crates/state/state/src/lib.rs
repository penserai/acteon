pub mod error;
pub mod key;
pub mod lock;
pub mod recurring;
pub mod store;
pub mod sync_version;
pub mod testing;

pub use error::StateError;
pub use key::{KeyKind, StateKey};
pub use lock::{DistributedLock, LockGuard};
pub use recurring::{
    recurring_active_counter_key, remove_pending_recurring, set_pending_recurring,
};
pub use store::{CasResult, StateStore};
pub use sync_version::{SyncDomain, bump_sync_version, read_sync_version, sync_version_key};
