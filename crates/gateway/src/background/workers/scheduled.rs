use tracing::{debug, info, warn};

use acteon_state::{CasResult, KeyKind, StateKey};

use super::super::{BackgroundProcessor, ScheduledActionDueEvent};
use crate::scheduled::{
    DELIVERY_LEASE_SECONDS, DeliveryLease, ScheduledRecord, cleanup_pending, pending_key,
    record_key,
};

impl BackgroundProcessor {
    /// Deliver due records under a durable lease. Discovery remains until the
    /// consumer commits an outcome; channel closure/cancellation cannot lose it.
    pub(crate) async fn process_scheduled_actions(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(tx) = &self.scheduled_action_tx else {
            return Ok(());
        };
        let due = self
            .state
            .get_expired_timeouts(self.clock.now().timestamp_millis())
            .await?;
        for key in due {
            let parts: Vec<_> = key.splitn(4, ':').collect();
            if parts.len() != 4 || parts[2] != "pending_scheduled" {
                continue;
            }
            if let Some(event) = self
                .claim_scheduled_action(parts[0], parts[1], parts[3])
                .await?
            {
                info!(action_id = %event.action_id, "handing off scheduled delivery");
                if tx.send(event).await.is_err() {
                    warn!(
                        "scheduled action channel closed; durable delivery will retry after its lease"
                    );
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn claim_scheduled_action(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<Option<ScheduledActionDueEvent>, Box<dyn std::error::Error + Send + Sync>> {
        // Honor outstanding claims written by the preceding worker version.
        let legacy_claim = StateKey::new(
            namespace,
            tenant,
            KeyKind::ScheduledAction,
            format!("{id}:claim"),
        );
        if self.state.get(&legacy_claim).await?.is_some() {
            return Ok(None);
        }
        let key = record_key(namespace, tenant, id);
        for _ in 0..5 {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                cleanup_pending(self.state.as_ref(), namespace, tenant, id).await?;
                return Ok(None);
            };
            let clear = match self.decrypt_state_value(&raw) {
                Ok(clear) => clear,
                Err(error) => {
                    warn!(%error, action_id = id, "unreadable scheduled record; retained for repair");
                    return Ok(None);
                }
            };
            let mut record: ScheduledRecord = match serde_json::from_str(&clear) {
                Ok(record) => record,
                Err(error) => {
                    warn!(%error, action_id = id, "invalid scheduled record; retained for repair");
                    return Ok(None);
                }
            };
            if let Err(error) = record.validate_scope(namespace, tenant, id) {
                warn!(%error, action_id = id, "invalid scheduled scope; retained for repair");
                return Ok(None);
            }
            if record.completed_at.is_some() {
                cleanup_pending(self.state.as_ref(), namespace, tenant, id).await?;
                return Ok(None);
            }
            let now = self.clock.now();
            if record.scheduled_for > now {
                self.state
                    .index_timeout(
                        &pending_key(namespace, tenant, id),
                        record.scheduled_for.timestamp_millis(),
                    )
                    .await?;
                return Ok(None);
            }
            if record
                .delivery
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now)
            {
                return Ok(None);
            }
            let token = uuid::Uuid::new_v4().to_string();
            record.delivery = Some(DeliveryLease {
                token: token.clone(),
                expires_at: now + chrono::Duration::seconds(i64::from(DELIVERY_LEASE_SECONDS)),
                started: false,
            });
            let raw = serde_json::to_string(&record)?;
            let encrypted = match &self.payload_encryptor {
                Some(enc) => enc.encrypt_str(&raw)?,
                None => raw,
            };
            if self
                .state
                .compare_and_swap(&key, version, &encrypted, None)
                .await?
                == CasResult::Ok
            {
                return Ok(Some(ScheduledActionDueEvent {
                    namespace: namespace.to_owned(),
                    tenant: tenant.to_owned(),
                    action_id: id.to_owned(),
                    delivery_token: token,
                    action: record.action,
                }));
            }
        }
        debug!(
            action_id = id,
            "scheduled delivery claim contention; retry next tick"
        );
        Ok(None)
    }

    /// Rebuild missing discovery entries from authoritative records, including
    /// interrupted initial writes and handoffs made by the previous worker.
    /// Runs on the cleanup cadence rather than on every due poll.
    pub(crate) async fn reconcile_scheduled_actions(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for (key, raw) in self
            .state
            .scan_keys_by_kind(KeyKind::ScheduledAction)
            .await?
        {
            let parts: Vec<_> = key.splitn(4, ':').collect();
            if parts.len() != 4 || parts[3].ends_with(":claim") {
                continue;
            }
            let clear = match self.decrypt_state_value(&raw) {
                Ok(clear) => clear,
                Err(error) => {
                    warn!(%error, action_id = parts[3], "unreadable scheduled record; retained for repair");
                    continue;
                }
            };
            let Ok(record) = serde_json::from_str::<ScheduledRecord>(&clear) else {
                continue;
            };
            if let Err(error) = record.validate_scope(parts[0], parts[1], parts[3]) {
                warn!(%error, action_id = parts[3], "invalid scheduled scope; retained for repair");
                continue;
            }
            if record.completed_at.is_some() {
                cleanup_pending(self.state.as_ref(), parts[0], parts[1], parts[3]).await?;
            } else {
                let key = pending_key(parts[0], parts[1], parts[3]);
                let due = record.scheduled_for.timestamp_millis();
                self.state.set(&key, &due.to_string(), None).await?;
                self.state.index_timeout(&key, due).await?;
            }
        }
        Ok(())
    }
}
