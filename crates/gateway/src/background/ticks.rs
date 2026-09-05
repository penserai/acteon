use std::time::Duration;

use super::{BackgroundConfig, BackgroundProcessor};

/// An independently drivable background worker cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundJob {
    GroupFlush,
    Timeout,
    ChainAdvance,
    ScheduledActions,
    RecurringActions,
    Retention,
    StaleTasks,
    TemplateSync,
    SilenceSync,
    TimeIntervalSync,
    GroupSync,
    Cleanup,
}

impl BackgroundJob {
    /// Stable ordering for simultaneous ticks in the production polling loop.
    pub const ALL: [Self; 12] = [
        Self::GroupFlush,
        Self::Timeout,
        Self::ChainAdvance,
        Self::ScheduledActions,
        Self::RecurringActions,
        Self::Retention,
        Self::StaleTasks,
        Self::TemplateSync,
        Self::SilenceSync,
        Self::TimeIntervalSync,
        Self::GroupSync,
        Self::Cleanup,
    ];

    pub(super) fn period(self, config: &BackgroundConfig) -> Option<Duration> {
        let (enabled, period) = match self {
            Self::GroupFlush => (config.enable_group_flush, config.group_flush_interval),
            Self::Timeout => (
                config.enable_timeout_processing,
                config.timeout_check_interval,
            ),
            Self::ChainAdvance => (config.enable_chain_advancement, config.chain_check_interval),
            Self::ScheduledActions => (
                config.enable_scheduled_actions,
                config.scheduled_check_interval,
            ),
            Self::RecurringActions => (
                config.enable_recurring_actions,
                config.recurring_check_interval,
            ),
            Self::Retention => (
                config.enable_retention_reaper,
                config.retention_check_interval,
            ),
            Self::StaleTasks => (
                config.enable_stale_task_reaper,
                config.stale_task_check_interval,
            ),
            Self::TemplateSync => (config.enable_template_sync, config.template_sync_interval),
            Self::SilenceSync => (config.enable_silence_sync, config.silence_sync_interval),
            Self::TimeIntervalSync => (
                config.enable_time_interval_sync,
                config.time_interval_sync_interval,
            ),
            Self::GroupSync => (config.enable_group_sync, config.group_sync_interval),
            Self::Cleanup => (true, config.cleanup_interval),
        };
        enabled.then_some(period)
    }
}

impl BackgroundProcessor {
    /// Execute one enabled worker cycle at the current clock time, without
    /// sleeping or advancing time. Disabled jobs are no-ops. This is the same
    /// dispatch used by [`Self::run`]; it does not alter that loop's cadence.
    ///
    /// Returned errors have the same meaning as worker errors in `run`;
    /// best-effort per-record failures remain logged by the workers. Channel
    /// consumers must be drained concurrently when a tick can fill a channel.
    /// Callers own ordering between ticks, mutations, and clock advances.
    pub async fn tick(
        &mut self,
        job: BackgroundJob,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if job.period(&self.config).is_none() {
            return Ok(());
        }
        match job {
            BackgroundJob::GroupFlush => self.flush_ready_groups().await?,
            BackgroundJob::Timeout => self.process_timeouts().await?,
            BackgroundJob::ChainAdvance => {
                // Try both independent duties even if the first fails.
                let chains = self.advance_pending_chains().await;
                let timers = if let Some(gw) = &self.gateway {
                    gw.read()
                        .await
                        .process_due_workflow_timers()
                        .await
                        .map(|_| ())
                } else {
                    Ok(())
                };
                chains?;
                timers?;
            }
            BackgroundJob::ScheduledActions => self.process_scheduled_actions().await?,
            BackgroundJob::RecurringActions => self.process_recurring_actions().await?,
            BackgroundJob::Retention => {
                let retention = self.run_retention_reaper().await;
                let pins = if let Some(gw) = &self.gateway {
                    gw.read().await.gc_pinned_definitions().await.map(|_| ())
                } else {
                    Ok(())
                };
                retention?;
                pins?;
            }
            BackgroundJob::StaleTasks => self.run_stale_task_reaper().await?,
            BackgroundJob::TemplateSync => {
                if let Some(gw) = &self.gateway {
                    gw.read().await.sync_templates_from_store().await?;
                }
            }
            BackgroundJob::SilenceSync => {
                if let Some(gw) = &self.gateway {
                    gw.read().await.sync_silences_from_store().await?;
                }
            }
            BackgroundJob::TimeIntervalSync => {
                if let Some(gw) = &self.gateway {
                    gw.read().await.sync_time_intervals_from_store().await?;
                }
            }
            BackgroundJob::GroupSync => {
                self.group_manager
                    .sync_groups_from_store(self.state.as_ref(), self.payload_encryptor.as_deref())
                    .await?;
            }
            BackgroundJob::Cleanup => self.run_cleanup().await?,
        }
        Ok(())
    }
}
