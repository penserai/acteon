use std::sync::atomic::Ordering;

use chrono::Utc;
use tracing::{debug, info, warn};

use acteon_state::{KeyKind, StateKey, recurring_active_counter_key};

use super::super::{BackgroundProcessor, RecurringActionDueEvent};

/// Number of recurring ticks between full reconciliation scans of the
/// pending-recurring index. On every other tick the `recurring_active` gauge is
/// refreshed from an O(1) read of the durable counter
/// (`recurring_active_counter_key`), which is maintained incrementally on each
/// index mutation. This removes the per-tick `scan_keys_by_kind` that, on
/// `DynamoDB`, was an unbounded full-table `Scan` whose RCU cost scaled with the
/// entire keyspace rather than the recurring workload (issue #118).
///
/// Tick 0 always reconciles, so the counter is seeded from ground truth at
/// startup; the periodic re-scan thereafter self-heals any drift left by a
/// crash between an index write and its counter bump. At the default 60s tick
/// this reconciles roughly hourly — ~1.6% of the previous scan volume.
const RECURRING_RECONCILE_EVERY_N_TICKS: u64 = 60;

impl BackgroundProcessor {
    /// Refresh the `recurring_active` gauge. Called once per recurring tick
    /// before dispatching due actions, so the gauge reflects the steady-state
    /// number of scheduled recurring actions even when nothing is due.
    ///
    /// Most ticks read the maintained durable counter (O(1) on every backend).
    /// Every [`RECURRING_RECONCILE_EVERY_N_TICKS`] ticks (and at startup) it
    /// falls back to an authoritative full scan, which also resets the counter
    /// to ground truth.
    pub(crate) async fn refresh_recurring_active_gauge(&self) {
        let tick = self.recurring_tick_count.fetch_add(1, Ordering::Relaxed);
        // `is_multiple_of` is true for tick 0, so the very first tick reconciles
        // (seeds the counter from a full scan) and then every Nth tick after.
        let reconcile = tick.is_multiple_of(RECURRING_RECONCILE_EVERY_N_TICKS);

        if reconcile {
            match self
                .state
                .scan_keys_by_kind(KeyKind::PendingRecurring)
                .await
            {
                Ok(entries) => {
                    let count = entries.len();
                    // Reset the durable counter to the scanned truth so future
                    // O(1) reads stay accurate.
                    let _ = self
                        .state
                        .set(&recurring_active_counter_key(), &count.to_string(), None)
                        .await;
                    self.metrics
                        .set_recurring_active(u64::try_from(count).unwrap_or(u64::MAX));
                }
                Err(e) => {
                    debug!(error = %e, "failed to reconcile recurring_active gauge");
                }
            }
            return;
        }

        match self.state.get(&recurring_active_counter_key()).await {
            Ok(Some(v)) => {
                if let Ok(n) = v.parse::<i64>() {
                    self.metrics
                        .set_recurring_active(u64::try_from(n.max(0)).unwrap_or(0));
                }
            }
            // Counter not yet materialized (no recurring action ever created).
            // Leave the gauge at its prior value; the next reconcile seeds it.
            Ok(None) => {}
            Err(e) => {
                debug!(error = %e, "failed to read recurring_active counter");
            }
        }
    }
}

impl BackgroundProcessor {
    /// Process recurring actions that are due for dispatch.
    ///
    /// Uses the timeout index to efficiently find expired `PendingRecurring`
    /// keys, loads the corresponding `RecurringAction` data, validates it is
    /// still active, and emits dispatch events.
    ///
    /// Uses an atomic claim key (`check_and_set`) to prevent double-dispatch
    /// when multiple server instances poll concurrently.
    ///
    /// After dispatch, computes the next occurrence and re-indexes the action.
    /// If the action has expired (`ends_at` in the past) or is disabled, it is
    /// removed from the pending index without dispatching.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn process_recurring_actions(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(ref tx) = self.recurring_action_tx else {
            return Ok(());
        };

        // Refresh the active gauge first so it stays current even when no
        // recurring actions are due in this tick.
        self.refresh_recurring_active_gauge().await;

        let now = Utc::now();
        let now_ms = now.timestamp_millis();

        let expired_keys = self.state.get_expired_timeouts(now_ms).await?;

        let due_keys: Vec<String> = expired_keys
            .into_iter()
            .filter(|k| k.contains(":pending_recurring:"))
            .collect();

        if due_keys.is_empty() {
            return Ok(());
        }

        let mut dispatched = 0u32;
        let mut skipped = 0u32;

        for key in due_keys {
            // Parse namespace:tenant:pending_recurring:recurring_id
            let parts: Vec<&str> = key.splitn(4, ':').collect();
            if parts.len() < 4 {
                // Malformed pending key is a genuine error — counts as
                // a recurring error so dashboards catch it. The key
                // parse is the worker's first sanity check and
                // failure here means the state store is corrupt or
                // under-version.
                warn!(key = %key, "invalid pending recurring key format");
                self.metrics.increment_recurring_errors();
                continue;
            }
            let namespace = parts[0];
            let tenant = parts[1];
            let recurring_id = parts[3];

            // Atomically claim this recurring action to prevent double-dispatch.
            let claim_key = StateKey::new(
                namespace,
                tenant,
                KeyKind::RecurringAction,
                format!("{recurring_id}:claim"),
            );
            let claimed = self
                .state
                .check_and_set(
                    &claim_key,
                    "claimed",
                    Some(std::time::Duration::from_secs(60)),
                )
                .await?;
            if !claimed {
                // Another replica grabbed this occurrence first. Normal
                // behavior under multi-replica CAS contention — record
                // it as a skip so capacity dashboards can show the
                // ratio, but don't treat it as an error.
                debug!(recurring_id = %recurring_id, "recurring action already claimed by another instance");
                self.metrics.increment_recurring_skipped();
                skipped += 1;
                continue;
            }

            // Load the recurring action data.
            let rec_key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, recurring_id);
            let Some(raw_str) = self.state.get(&rec_key).await? else {
                // Already deleted, clean up pending key.
                let pending_key =
                    StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
                acteon_state::remove_pending_recurring(self.state.as_ref(), &pending_key).await?;
                continue;
            };

            let data_str = match self.decrypt_state_value(&raw_str) {
                Ok(v) => v,
                Err(e) => {
                    // Encrypted state couldn't be opened — wrong
                    // master key, corrupt ciphertext, or key rotation
                    // in progress without re-encryption. Operators
                    // must notice this; bump the error counter so
                    // the Grafana error-rate alert fires.
                    warn!(recurring_id = %recurring_id, error = %e, "failed to decrypt recurring action data");
                    self.metrics.increment_recurring_errors();
                    continue;
                }
            };

            let Ok(recurring) = serde_json::from_str::<acteon_core::RecurringAction>(&data_str)
            else {
                // Stored JSON doesn't match the current
                // `RecurringAction` schema. Either a malformed write
                // got through or we're running a binary older than
                // the state it's reading. Either way, an operator
                // needs to see it.
                warn!(recurring_id = %recurring_id, "failed to deserialize recurring action");
                self.metrics.increment_recurring_errors();
                continue;
            };

            // Validate the action is still eligible for dispatch.
            let should_skip =
                !recurring.enabled || recurring.ends_at.is_some_and(|ends| ends <= now);

            if should_skip {
                debug!(
                    recurring_id = %recurring_id,
                    enabled = recurring.enabled,
                    "skipping ineligible recurring action"
                );
                // Remove from pending index so it won't be re-polled.
                let pending_key =
                    StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
                acteon_state::remove_pending_recurring(self.state.as_ref(), &pending_key).await?;
                self.metrics.increment_recurring_skipped();
                skipped += 1;
                continue;
            }

            // Idempotency check: if last_executed_at is very close to now,
            // skip to avoid double-dispatch (addresses security review R1).
            if let Some(last) = recurring.last_executed_at {
                let gap = (now - last).num_seconds();
                if gap < 5 {
                    debug!(
                        recurring_id = %recurring_id,
                        last_executed_secs_ago = gap,
                        "skipping recently-executed recurring action"
                    );
                    self.metrics.increment_recurring_skipped();
                    skipped += 1;
                    continue;
                }
            }

            // Advance the schedule to the next occurrence BEFORE handing off
            // the dispatch. The dispatch (consumer-side) can outlive the 60s
            // claim TTL — a chain, an approval, or a slow webhook — and until
            // the consumer re-indexes after it returns, this occurrence stays
            // "due" with a stale `last_executed_at`. A poll in that window
            // would re-claim (the TTL has lapsed) and dispatch the SAME
            // occurrence again. Re-arming the index here moves the entry past
            // `now`, so only genuinely-later occurrences can re-fire; the
            // consumer still performs the authoritative state update after
            // dispatch (and removes the entry if the action becomes inactive).
            // Missed occurrences are not backfilled, matching the contract —
            // and unlike a delete-before-dispatch, a consumer crash leaves the
            // action armed for its next occurrence rather than stranded.
            let pending_key =
                StateKey::new(namespace, tenant, KeyKind::PendingRecurring, recurring_id);
            // Re-arm ONLY if the action remains active after THIS dispatch,
            // mirroring the consumer's post-dispatch `still_active` check but
            // using the pre-increment count + 1 (this dispatch is the
            // `execution_count + 1`-th). Without the max/ends bound, a consumer
            // crash on the final occurrence would leave a post-final entry
            // armed and over-fire it once; with it, the final occurrence drops
            // the index so nothing further can be polled.
            let rearm_to = acteon_core::validate_cron_expr(&recurring.cron_expr)
                .ok()
                .and_then(|cron| {
                    acteon_core::validate_timezone(&recurring.timezone)
                        .ok()
                        .and_then(|tz| acteon_core::next_occurrence(&cron, tz, &now))
                })
                .filter(|next| {
                    recurring.ends_at.is_none_or(|ends| *next <= ends)
                        && recurring
                            .max_executions
                            .is_none_or(|max| recurring.execution_count + 1 < max)
                });
            if let Some(next) = rearm_to {
                // Re-arming an entry that is already in the index leaves the
                // active count unchanged (the helper only counts true
                // insertions).
                acteon_state::set_pending_recurring(
                    self.state.as_ref(),
                    &pending_key,
                    next.timestamp_millis(),
                )
                .await?;
            } else {
                // No further occurrence (exhausted, past ends_at, at max, or
                // unparsable) — drop it from the index.
                acteon_state::remove_pending_recurring(self.state.as_ref(), &pending_key).await?;
            }

            info!(
                recurring_id = %recurring_id,
                namespace = %namespace,
                tenant = %tenant,
                cron_expr = %recurring.cron_expr,
                "dispatching recurring action"
            );

            let event = RecurringActionDueEvent {
                namespace: namespace.to_string(),
                tenant: tenant.to_string(),
                recurring_id: recurring_id.to_string(),
                recurring_action: recurring,
            };

            if tx.send(event).await.is_err() {
                warn!("recurring action event channel closed");
                return Ok(());
            }
            // The worker has successfully handed this occurrence to the
            // consumer; the consumer increments `recurring_dispatched` once the
            // action actually fires. The gap between the two is the in-flight /
            // stuck depth (issue #122).
            self.metrics.increment_recurring_events_emitted();
            dispatched += 1;
        }

        if dispatched > 0 || skipped > 0 {
            debug!(dispatched, skipped, "processed recurring actions");
        }

        Ok(())
    }
}
