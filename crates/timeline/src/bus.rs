//! Acknowledged transport adapter; offsets advance only after the shared ingestion boundary.
use crate::{Error, Result, TimelineScope, TimelineService, config::SourceConfig};
use acteon_bus::{AcknowledgedSubscription, BusBackend, StartOffset, SubscriptionConfig};
use std::sync::Arc;

pub async fn consume(
    service: Arc<TimelineService>,
    backend: &dyn BusBackend,
    source: &SourceConfig,
) -> Result<()> {
    let mut subscription = backend
        .subscribe_acknowledged(
            &source.topic,
            &source.consumer_group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
    let scope = TimelineScope {
        namespace: source.namespace.clone(),
        tenant: source.tenant.clone(),
    };
    loop {
        let first = subscription
            .recv()
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut deliveries = vec![first];
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(5);
        while deliveries.len() < 128 {
            match tokio::time::timeout_at(deadline, subscription.recv()).await {
                Ok(Ok(delivery)) => deliveries.push(delivery),
                Ok(Err(error)) => return Err(Error::Storage(error.to_string())),
                Err(_) => break,
            }
        }
        let receipts: Vec<_> = deliveries.iter().map(|d| d.receipt.clone()).collect();
        subscription
            .validate_receipts(&receipts)
            .map_err(|e| Error::Storage(e.to_string()))?;
        // Admission is independently version-fenced. Failed batches leave every offset
        // uncommitted, and successfully persisted members are idempotent on redelivery.
        futures::future::try_join_all(
            deliveries
                .into_iter()
                .map(|d| process(&service, source, &scope, d.message, d.receipt.position())),
        )
        .await?;
        subscription
            .acknowledge(&receipts)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
    }
}
pub async fn consume_one(
    service: &TimelineService,
    subscription: &mut dyn AcknowledgedSubscription,
    source: &SourceConfig,
    scope: &TimelineScope,
) -> Result<()> {
    let delivery = subscription
        .recv()
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
    process(
        service,
        source,
        scope,
        delivery.message,
        delivery.receipt.position(),
    )
    .await?;
    subscription
        .acknowledge(&[delivery.receipt])
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}
async fn process(
    service: &TimelineService,
    source: &SourceConfig,
    scope: &TimelineScope,
    message: acteon_bus::BusMessage,
    position: acteon_bus::OffsetPosition,
) -> Result<()> {
    let decode = || -> Result<Option<crate::TimelineChange>> {
        Ok(if let Some(mapping) = &source.debezium {
            let key = message
                .key
                .as_ref()
                .map(|k| serde_json::from_str(k))
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            mapping.decode(
                &source.source,
                &key,
                &message.payload,
                &source.topic,
                position.partition,
                position.offset,
            )?
        } else {
            Some(serde_json::from_value::<crate::TimelineChange>(
                message.payload.clone(),
            )?)
        })
    };
    let change = match decode() {
        Ok(change) => change,
        Err(error) => return poison(service, source, scope, &message, position, error).await,
    };
    if let Some(change) = change {
        if source.debezium.is_none() && change.source != source.source {
            return Err(Error::Invalid(
                "source identity does not match connector binding".into(),
            ));
        }
        // Bind the event identity to its original Kafka partition. Never compare offsets across lanes.
        let key = acteon_state::StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            acteon_state::KeyKind::Custom("timeline_lane".into()),
            crate::store::identity(scope, &change.source, &change.event_id),
        );
        let lane = serde_json::to_string(&(&source.topic, position.partition))?;
        if !service.state.check_and_set(&key, &lane, None).await?
            && service.state.get(&key).await?.as_deref() != Some(&lane)
        {
            return Err(Error::Invalid(
                "event moved Kafka lanes; register a new source generation".into(),
            ));
        }
        let transport = serde_json::json!({"topic":source.topic,"partition":position.partition,"offset":position.offset,"key":message.key});
        let mut retry = std::time::Duration::from_millis(100);
        loop {
            match service
                .ingest_raw(
                    scope,
                    change.clone(),
                    message.payload.clone(),
                    Some(transport.clone()),
                )
                .await
            {
                Ok(_) => break,
                Err(error @ (Error::Invalid(_) | Error::Conflict | Error::Json(_))) => {
                    return poison(service, source, scope, &message, position, error).await;
                }
                Err(error) => {
                    tracing::warn!(error=%error,"timeline ingestion retry; source offset remains uncommitted");
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(std::time::Duration::from_secs(30));
                }
            }
        }
    }
    Ok(())
}

async fn poison(
    service: &TimelineService,
    source: &SourceConfig,
    scope: &TimelineScope,
    message: &acteon_bus::BusMessage,
    position: acteon_bus::OffsetPosition,
    error: Error,
) -> Result<()> {
    if source.poison_policy == crate::config::PoisonPolicy::Halt {
        return Err(error);
    }
    let reason = match error {
        Error::Conflict => "revision_conflict",
        Error::Json(_) => "decode_failure",
        _ => "validation_failure",
    };
    let binding = source.debezium.as_ref().map_or_else(
        || source.source.clone(),
        |mapping| format!("{}@{}", source.source, mapping.generation),
    );
    let transport = serde_json::json!({"source":binding,"topic":source.topic,"partition":position.partition,"offset":position.offset,"key":message.key,"rejection":reason});
    service
        .quarantine(scope, &binding, message.payload.clone(), transport)
        .await
}
