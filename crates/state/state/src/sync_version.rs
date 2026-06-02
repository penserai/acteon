//! Cache-sync version counters for cheap change detection.
//!
//! Several gateway caches (payload templates, silences, time intervals) are
//! kept consistent across HA nodes by a periodic background sync that calls
//! `scan_keys_by_kind` and rebuilds the in-memory map. On Redis that scan is a
//! full-keyspace `SCAN`, on Postgres/DynamoDB a full-table read — O(total
//! keyspace), paid by every node on every tick even when nothing changed.
//!
//! These resources are admin-managed and change rarely relative to the sync
//! interval, so instead of scanning every tick we keep a single monotonic
//! version counter per domain, bumped on every mutating write. A node reads
//! that counter (an O(1) `get`) each tick and only performs the expensive
//! scan+rebuild when the version differs from the one it last synced.
//!
//! ## Ordering contract
//!
//! Callers MUST complete the data write (the `set`/`delete` of the resource)
//! **before** calling [`bump_sync_version`]. That ordering guarantees that any
//! node which observes a given version has the corresponding data visible to
//! its subsequent scan, so recording the observed version can never skip an
//! unseen change. Bumps are otherwise best-effort: a lost bump (e.g. a
//! transient store error after the data write) is self-healed by the sync's
//! periodic full-reconcile fallback.

use crate::key::{KeyKind, StateKey};
use crate::{StateError, StateStore};

/// A gateway cache whose freshness is gated by a sync-version counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncDomain {
    /// Payload templates and template profiles (`Template` + `TemplateProfile`).
    Templates,
    /// Notification silences (`Silence`).
    Silences,
    /// Named time intervals (`TimeInterval`).
    TimeIntervals,
}

impl SyncDomain {
    /// Stable identifier embedded in the counter key. Changing these strings
    /// resets every node's view (harmless — the next sync re-scans).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Templates => "templates",
            Self::Silences => "silences",
            Self::TimeIntervals => "time_intervals",
        }
    }
}

/// Canonical key for a domain's sync-version counter. Addressed under
/// `_system`/`_global` so it sits outside any tenant keyspace and outside the
/// scanned resource kinds.
#[must_use]
pub fn sync_version_key(domain: SyncDomain) -> StateKey {
    StateKey::new(
        "_system",
        "_global",
        KeyKind::Custom(format!("sync_version:{}", domain.as_str())),
        "v",
    )
}

/// Bump a domain's sync version after a mutating write. Call this only once the
/// data write has committed (see the module ordering contract).
///
/// # Errors
/// Returns the underlying [`StateError`] if the counter increment fails.
pub async fn bump_sync_version(
    state: &dyn StateStore,
    domain: SyncDomain,
) -> Result<i64, StateError> {
    state.increment(&sync_version_key(domain), 1, None).await
}

/// Read a domain's current sync version, treating a missing counter as `0`
/// (no mutating write has happened yet).
///
/// # Errors
/// Returns the underlying [`StateError`] if the read fails.
pub async fn read_sync_version(
    state: &dyn StateStore,
    domain: SyncDomain,
) -> Result<i64, StateError> {
    Ok(state
        .get(&sync_version_key(domain))
        .await?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0))
}
