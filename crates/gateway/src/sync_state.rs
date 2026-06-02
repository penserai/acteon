//! Per-node tracking of the last cache-sync version observed for each
//! [`SyncDomain`], so the periodic background sync workers can skip the
//! expensive `scan_keys_by_kind` rebuild when nothing has changed.
//!
//! See [`acteon_state::sync_version`] for the durable counter side. This struct
//! is the in-process companion: it remembers the version a node last fully
//! synced and decides, each tick, whether a fresh scan is warranted.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use acteon_state::SyncDomain;

/// How many sync ticks between forced full-reconcile scans, regardless of the
/// version gate. This self-heals the rare case of a version bump lost to a
/// transient state-store error after its data write committed: a stale node
/// converges within at most this many ticks instead of waiting for the next
/// mutating write. At the default intervals that is ~10 min for silences (10s
/// ticks) and ~30 min for templates/time intervals (30s ticks).
const SYNC_RECONCILE_EVERY_N_TICKS: u64 = 60;

/// Per-domain sync bookkeeping for one gateway instance.
#[derive(Debug, Default)]
pub(crate) struct SyncVersionTracker {
    templates: Slot,
    silences: Slot,
    time_intervals: Slot,
}

#[derive(Debug)]
struct Slot {
    /// Store version this node last fully synced. Initialised to `-1` (an
    /// impossible store value, since versions are `>= 0`) so the first sync
    /// always runs and loads any data already present.
    last_version: AtomicI64,
    /// Monotonic count of sync attempts, driving the periodic reconcile.
    ticks: AtomicU64,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            last_version: AtomicI64::new(-1),
            ticks: AtomicU64::new(0),
        }
    }
}

impl SyncVersionTracker {
    fn slot(&self, domain: SyncDomain) -> &Slot {
        match domain {
            SyncDomain::Templates => &self.templates,
            SyncDomain::Silences => &self.silences,
            SyncDomain::TimeIntervals => &self.time_intervals,
        }
    }

    /// Decide whether a full scan+rebuild should run this tick for `domain`,
    /// given the `current_version` just read from the store. Returns `true`
    /// when the version differs from the last fully-synced one, or a periodic
    /// reconcile is due. Always `true` on the first call (last version `-1`,
    /// tick `0` is a multiple of the reconcile interval). Advances the tick
    /// counter as a side effect, so call exactly once per sync attempt.
    pub(crate) fn should_sync(&self, domain: SyncDomain, current_version: i64) -> bool {
        let slot = self.slot(domain);
        let tick = slot.ticks.fetch_add(1, Ordering::Relaxed);
        current_version != slot.last_version.load(Ordering::Relaxed)
            || tick.is_multiple_of(SYNC_RECONCILE_EVERY_N_TICKS)
    }

    /// Record the store version observed by a completed full sync.
    pub(crate) fn record(&self, domain: SyncDomain, version: i64) {
        self.slot(domain)
            .last_version
            .store(version, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_always_syncs_then_gates_on_version() {
        let t = SyncVersionTracker::default();
        // First call: forced (last = -1, tick 0).
        assert!(t.should_sync(SyncDomain::Silences, 0));
        t.record(SyncDomain::Silences, 0);
        // Unchanged version, not a reconcile tick → skip.
        assert!(!t.should_sync(SyncDomain::Silences, 0));
        // Version advanced by a peer write → sync.
        assert!(t.should_sync(SyncDomain::Silences, 1));
        t.record(SyncDomain::Silences, 1);
        assert!(!t.should_sync(SyncDomain::Silences, 1));
    }

    #[test]
    fn domains_are_independent() {
        let t = SyncVersionTracker::default();
        assert!(t.should_sync(SyncDomain::Templates, 0));
        t.record(SyncDomain::Templates, 0);
        assert!(!t.should_sync(SyncDomain::Templates, 0));
        // A different domain still forces its own first sync.
        assert!(t.should_sync(SyncDomain::TimeIntervals, 0));
    }

    #[test]
    fn periodic_reconcile_fires_even_without_version_change() {
        let t = SyncVersionTracker::default();
        // Consume the forced first tick.
        assert!(t.should_sync(SyncDomain::Templates, 5));
        t.record(SyncDomain::Templates, 5);
        let mut forced = 0;
        // Over a full reconcile period with no version change, at least one
        // forced scan must occur.
        for _ in 0..SYNC_RECONCILE_EVERY_N_TICKS {
            if t.should_sync(SyncDomain::Templates, 5) {
                forced += 1;
            }
        }
        assert!(
            forced >= 1,
            "expected a periodic reconcile within the window"
        );
    }
}
