//! Durable, leased handoff of scheduled actions to the dispatch pipeline.

use std::time::Duration;

use acteon_core::{Action, ActionOutcome};
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::background::ScheduledActionDueEvent;
use crate::gateway::DispatchOrigin;
use crate::{Gateway, GatewayError};

pub(crate) const DELIVERY_LEASE: Duration = Duration::from_secs(DELIVERY_LEASE_SECONDS as u64);
pub(crate) const DELIVERY_LEASE_SECONDS: u32 = 60;
const COMPLETED_TTL: Duration = Duration::from_secs(86_400);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ScheduledRecord {
    pub action_id: String,
    pub action: Action,
    pub scheduled_for: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub delivery: Option<DeliveryLease>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub outcome: Option<ActionOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeliveryLease {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub started: bool,
}

impl ScheduledRecord {
    pub(crate) fn validate_scope(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<(), GatewayError> {
        if self.action_id != id
            || self.action.namespace.as_str() != namespace
            || self.action.tenant.as_str() != tenant
        {
            return Err(GatewayError::Configuration(
                "scheduled action scope does not match its storage key".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn record_key(namespace: &str, tenant: &str, id: &str) -> StateKey {
    StateKey::new(namespace, tenant, KeyKind::ScheduledAction, id)
}

pub(crate) fn pending_key(namespace: &str, tenant: &str, id: &str) -> StateKey {
    StateKey::new(namespace, tenant, KeyKind::PendingScheduled, id)
}

pub(crate) async fn cleanup_pending(
    state: &dyn StateStore,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<(), GatewayError> {
    let key = pending_key(namespace, tenant, id);
    state.delete(&key).await?;
    state.remove_timeout_index(&key).await?;
    Ok(())
}

enum DeliveryMutation<'a> {
    Start,
    Renew,
    Complete(&'a ActionOutcome),
}

impl Gateway {
    /// Consume one leased scheduled delivery. The action is loaded from durable
    /// state; the event's payload is never treated as authority. Expired,
    /// replaced, duplicate, and completed deliveries return `Ok(None)`.
    ///
    /// Renews ownership while dispatch is pending, then persists its outcome
    /// before cleaning discovery indexes. Dispatch errors or cancellation leave
    /// the record discoverable for redelivery after lease expiry. External
    /// effects can repeat after a crash before outcome persistence; providers
    /// must use downstream idempotency when duplicate effects are unacceptable.
    pub async fn dispatch_scheduled_action(
        &self,
        event: &ScheduledActionDueEvent,
    ) -> Result<Option<ActionOutcome>, GatewayError> {
        let Some(record) = self
            .mutate_scheduled_delivery(event, DeliveryMutation::Start)
            .await?
        else {
            return Ok(None);
        };
        let _ = self.stream_tx.send(acteon_core::StreamEvent {
            id: uuid::Uuid::now_v7().to_string(),
            timestamp: self.clock.now(),
            event_type: acteon_core::StreamEventType::ScheduledActionDue {
                action_id: event.action_id.clone(),
            },
            namespace: event.namespace.clone(),
            tenant: event.tenant.clone(),
            action_type: Some(record.action.action_type.clone()),
            action_id: Some(event.action_id.clone()),
        });
        let dispatch = self.dispatch_inner(record.action, None, false, DispatchOrigin::Scheduled);
        tokio::pin!(dispatch);
        let outcome = loop {
            tokio::select! {
                biased;
                result = &mut dispatch => break result?,
                () = self.clock.sleep(DELIVERY_LEASE / 3) => {
                    if self.mutate_scheduled_delivery(event, DeliveryMutation::Renew).await?.is_none() {
                        return Err(GatewayError::Configuration("scheduled delivery lease lost during dispatch".into()));
                    }
                }
            }
        };
        if self
            .mutate_scheduled_delivery(event, DeliveryMutation::Complete(&outcome))
            .await?
            .is_none()
        {
            return Err(GatewayError::Configuration(
                "scheduled delivery lease lost before acknowledgement".into(),
            ));
        }
        if let Err(error) = cleanup_pending(
            self.state.as_ref(),
            &event.namespace,
            &event.tenant,
            &event.action_id,
        )
        .await
        {
            tracing::warn!(%error, action_id = %event.action_id, "scheduled outcome persisted; index cleanup will retry");
        }
        Ok(Some(outcome))
    }

    async fn mutate_scheduled_delivery(
        &self,
        event: &ScheduledActionDueEvent,
        mutation: DeliveryMutation<'_>,
    ) -> Result<Option<ScheduledRecord>, GatewayError> {
        let key = record_key(&event.namespace, &event.tenant, &event.action_id);
        for _ in 0..5 {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Ok(None);
            };
            let mut record: ScheduledRecord =
                serde_json::from_str(&self.decrypt_state_value(&raw)?).map_err(|e| {
                    GatewayError::Configuration(format!("invalid scheduled record: {e}"))
                })?;
            record.validate_scope(&event.namespace, &event.tenant, &event.action_id)?;
            let now = self.clock.now();
            if record.completed_at.is_some() || record.scheduled_for > now {
                return Ok(None);
            }
            let Some(lease) = &mut record.delivery else {
                return Ok(None);
            };
            if lease.token != event.delivery_token || now >= lease.expires_at {
                return Ok(None);
            }
            match mutation {
                DeliveryMutation::Start => {
                    if lease.started {
                        return Ok(None);
                    }
                    lease.started = true;
                    lease.expires_at =
                        now + chrono::Duration::seconds(i64::from(DELIVERY_LEASE_SECONDS));
                }
                DeliveryMutation::Renew => {
                    if !lease.started {
                        return Ok(None);
                    }
                    lease.expires_at =
                        now + chrono::Duration::seconds(i64::from(DELIVERY_LEASE_SECONDS));
                }
                DeliveryMutation::Complete(outcome) => {
                    if !lease.started {
                        return Ok(None);
                    }
                    record.completed_at = Some(now);
                    record.outcome = Some(outcome.clone());
                }
            }
            let ttl = record.completed_at.map(|_| COMPLETED_TTL);
            let raw = serde_json::to_string(&record)
                .map_err(|e| GatewayError::Configuration(e.to_string()))?;
            let encrypted = self.encrypt_state_value(&raw)?;
            if self
                .state
                .compare_and_swap(&key, version, &encrypted, ttl)
                .await?
                == CasResult::Ok
            {
                return Ok(Some(record));
            }
        }
        Err(GatewayError::Configuration(
            "scheduled delivery CAS contention exhausted".into(),
        ))
    }
}
