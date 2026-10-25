//! `PostgreSQL` Debezium JSON adapter. Ordering uses a stable Kafka lane, never wall-clock time.
use crate::{ChangeOperation, Error, EventBody, Result, TimelineChange};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebeziumMapping {
    /// JSON Pointers into the row image.
    pub event_group: String,
    pub occurred_at: String,
    pub event_type: String,
    pub message: Option<String>,
    /// If omitted, the full row is retained as attributes.
    pub attributes: Option<String>,
    /// New source identity required when a topic is reset or repartitioned.
    pub generation: String,
}
impl DebeziumMapping {
    fn body(&self, row: &Value) -> Result<EventBody> {
        let string = |pointer: &str| -> Result<String> {
            row.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| Error::Invalid(format!("missing string at {pointer}")))
        };
        Ok(EventBody {
            event_group: string(&self.event_group)?,
            occurred_at: chrono::DateTime::parse_from_rfc3339(&string(&self.occurred_at)?)
                .map_err(|_| Error::Invalid("occurredAt must be RFC3339 text".into()))?
                .into(),
            event_type: string(&self.event_type)?,
            message: self.message.as_ref().map(|p| string(p)).transpose()?,
            attributes: self.attributes.as_ref().map_or_else(
                || Ok(row.clone()),
                |p| {
                    row.pointer(p)
                        .cloned()
                        .ok_or_else(|| Error::Invalid("missing attributes".into()))
                },
            )?,
        })
    }
    pub fn decode(
        &self,
        source: &str,
        key: &Value,
        payload: &Value,
        topic: &str,
        partition: i32,
        offset: i64,
    ) -> Result<Option<TimelineChange>> {
        if payload.is_null() {
            return Ok(None);
        }
        if key.is_null() || partition < 0 || offset < 0 || self.generation.is_empty() {
            return Err(Error::Invalid(
                "Debezium requires key, source generation, and valid Kafka position".into(),
            ));
        }
        let envelope = payload.get("payload").unwrap_or(payload);
        if envelope
            .pointer("/source/connector")
            .and_then(Value::as_str)
            != Some("postgresql")
        {
            return Err(Error::Invalid(
                "only Debezium PostgreSQL is supported".into(),
            ));
        }
        let operation = match envelope["op"].as_str() {
            Some("c") => ChangeOperation::Create,
            Some("u") => ChangeOperation::Update,
            Some("d") => ChangeOperation::Delete,
            Some("r") => ChangeOperation::Snapshot,
            _ => return Err(Error::Invalid("unsupported Debezium operation".into())),
        };
        let after = if operation == ChangeOperation::Delete {
            None
        } else {
            Some(self.body(&envelope["after"])?)
        };
        // Replica identity may supply only primary keys in before; preserve raw envelope in ledger.
        let before = envelope
            .get("before")
            .filter(|v| !v.is_null())
            .and_then(|v| self.body(v).ok());
        let key = key.get("payload").unwrap_or(key);
        let revision = u64::try_from(offset)
            .map_err(|_| Error::Invalid("invalid offset".into()))?
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("offset overflow".into()))?;
        Ok(Some(TimelineChange {
            schema_version: 1,
            change_id: crate::digest(&(source, &self.generation, topic, partition, offset)),
            source: format!("{source}@{}", self.generation),
            event_id: crate::digest(key),
            revision,
            operation,
            before,
            after,
        }))
    }
}
