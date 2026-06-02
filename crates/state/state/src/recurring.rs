//! Pending-recurring index maintenance with a durable active-count counter.
//!
//! The `recurring_active` gauge used to be refreshed every background tick by
//! a full `scan_keys_by_kind(PendingRecurring)` — on `DynamoDB` an unbounded
//! full-table `Scan` whose RCU cost scales with the *entire* keyspace, not the
//! recurring workload (issue #118). To avoid that per-tick scan, every mutation
//! of the pending-recurring index goes through the helpers here, which keep a
//! single globally-shared durable counter in sync. The worker then refreshes
//! the gauge from an O(1) read of that counter, reconciling against a full
//! scan only occasionally to self-heal drift.
//!
//! The counter is maintained by *set-membership transitions*, not by raw
//! operation counts:
//! - [`set_pending_recurring`] bumps the counter `+1` only when the entry was
//!   newly added (absent → present), detected with one O(1) `get`. Re-arming an
//!   already-pending entry to its next occurrence leaves the count unchanged.
//! - [`remove_pending_recurring`] decrements `-1` only when the entry actually
//!   existed (present → absent), using the boolean returned by `delete`.
//!
//! Applied uniformly at every site that touches the index (server CRUD, the
//! post-dispatch consumer, and the worker's re-arm/drop paths), this stays
//! correct even across the worker→consumer double-mutation of the same key
//! around a single dispatch. Counter bumps are best-effort: a failed bump (or a
//! crash between the index write and the bump) self-heals on the next
//! reconciliation scan, so it never fails the underlying index operation.

use crate::key::{KeyKind, StateKey};
use crate::{StateError, StateStore};

/// Canonical key for the durable, globally-shared counter that tracks the
/// number of active `PendingRecurring` index entries across all namespaces and
/// tenants. The fixed `_system`/`_global` addressing keeps it outside any real
/// tenant keyspace and outside the `PendingRecurring` kind, so it is never
/// counted by `scan_keys_by_kind(PendingRecurring)`.
#[must_use]
pub fn recurring_active_counter_key() -> StateKey {
    StateKey::new(
        "_system",
        "_global",
        KeyKind::Custom("recurring_active_count".to_owned()),
        "all",
    )
}

/// Add or re-arm a `PendingRecurring` index entry at `next_ms`, keeping the
/// durable active-count counter in sync. Increments the counter only on a true
/// insertion (absent → present). See the module docs for the rationale.
///
/// # Errors
/// Returns the underlying [`StateError`] if the `set` or timeout-index write
/// fails. The counter bump is best-effort and never surfaces as an error.
pub async fn set_pending_recurring(
    state: &dyn StateStore,
    pending_key: &StateKey,
    next_ms: i64,
) -> Result<(), StateError> {
    let was_absent = state.get(pending_key).await?.is_none();
    state.set(pending_key, &next_ms.to_string(), None).await?;
    state.index_timeout(pending_key, next_ms).await?;
    if was_absent {
        let _ = state
            .increment(&recurring_active_counter_key(), 1, None)
            .await;
    }
    Ok(())
}

/// Remove a `PendingRecurring` index entry, keeping the durable active-count
/// counter in sync. Decrements the counter only when the entry actually existed
/// (present → absent), using `delete`'s returned boolean.
///
/// # Errors
/// Returns the underlying [`StateError`] if the `delete` fails. The
/// timeout-index removal and counter bump are best-effort.
pub async fn remove_pending_recurring(
    state: &dyn StateStore,
    pending_key: &StateKey,
) -> Result<(), StateError> {
    let existed = state.delete(pending_key).await?;
    let _ = state.remove_timeout_index(pending_key).await;
    if existed {
        let _ = state
            .increment(&recurring_active_counter_key(), -1, None)
            .await;
    }
    Ok(())
}

// Unit/integration tests for these helpers live in `acteon-gateway`
// (crates/gateway/src/background/mod.rs), which has access to a concrete
// `MemoryStateStore`. `acteon-state` cannot depend on `acteon-state-memory`
// here without a build cycle.
