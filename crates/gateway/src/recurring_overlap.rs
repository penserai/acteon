//! Gateway-level enforcement of recurring-action overlap policies.
//!
//! Overlap only matters when an occurrence spawns a chain execution (plain
//! provider dispatches complete synchronously and never overlap), and the
//! chain engine lives here — so the enforcement decision does too. Any
//! consumer dispatching recurring occurrences (the bundled server's
//! consumer, or an embedder driving the gateway directly) calls
//! [`Gateway::enforce_overlap_policy`] before dispatch and honors the
//! returned [`OverlapDecision`].

use tracing::{debug, info};

use acteon_core::{OverlapPolicy, RecurringAction};

use crate::gateway::Gateway;

/// Decision for one recurring occurrence under its overlap policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlapDecision {
    /// Dispatch the occurrence.
    Proceed,
    /// The execution spawned by the previous occurrence is still running
    /// and the policy is `skip`: drop this occurrence. The caller keeps
    /// the schedule alive (the next occurrence is already re-armed) but
    /// must not dispatch or count this one as executed.
    Skip {
        /// The still-active previous execution.
        previous_execution_id: String,
    },
}

impl Gateway {
    /// Enforce a recurring action's overlap policy ahead of dispatching an
    /// occurrence.
    ///
    /// - `allow_all` (default): always [`OverlapDecision::Proceed`].
    /// - `skip`: while the chain execution spawned by the previous
    ///   occurrence is still active, return [`OverlapDecision::Skip`]
    ///   (counted in the `recurring_skipped` metric).
    /// - `cancel_other`: cancel the still-active previous execution
    ///   (best-effort), then proceed.
    ///
    /// Enforcement is best-effort by design: a failed status read proceeds
    /// (an overlapping dispatch beats silently stalling the schedule), and
    /// a failed cancel proceeds (the execution may have settled between
    /// the status read and the cancel).
    pub async fn enforce_overlap_policy(
        &self,
        namespace: &str,
        tenant: &str,
        recurring_id: &str,
        recurring: &RecurringAction,
    ) -> OverlapDecision {
        if recurring.overlap_policy == OverlapPolicy::AllowAll {
            return OverlapDecision::Proceed;
        }
        let Some(prev_id) = recurring.last_execution_id.as_deref() else {
            return OverlapDecision::Proceed;
        };
        let still_active = matches!(
            self.get_chain_status(namespace, tenant, prev_id).await,
            Ok(Some(prev)) if prev.status.is_active()
        );
        if !still_active {
            return OverlapDecision::Proceed;
        }
        match recurring.overlap_policy {
            OverlapPolicy::Skip => {
                info!(
                    recurring_id,
                    previous_execution = prev_id,
                    "previous execution still running; occurrence skipped (overlap policy)"
                );
                self.metrics().increment_recurring_skipped();
                OverlapDecision::Skip {
                    previous_execution_id: prev_id.to_owned(),
                }
            }
            OverlapPolicy::CancelOther => {
                info!(
                    recurring_id,
                    previous_execution = prev_id,
                    "cancelling still-running previous execution (overlap policy)"
                );
                if let Err(e) = self
                    .cancel_chain(
                        namespace,
                        tenant,
                        prev_id,
                        Some("superseded by next recurring occurrence".to_owned()),
                        Some(format!("recurring:{recurring_id}")),
                    )
                    .await
                {
                    // The execution may have settled between the status
                    // read and the cancel — proceed.
                    debug!(
                        previous_execution = prev_id,
                        error = %e,
                        "overlap cancel failed (execution may have settled)"
                    );
                }
                OverlapDecision::Proceed
            }
            OverlapPolicy::AllowAll => unreachable!(),
        }
    }
}
