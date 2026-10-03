//! Pinned consume contracts and bounded, checkpoint-atomic poison-record retention.
use crate::{BusMessage, StreamPosition};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamInputContract {
    pub namespace: String,
    pub tenant: String,
    pub subject: String,
    pub version: i32,
    pub body: Value,
    pub sha256: String,
}
impl StreamInputContract {
    pub fn from_schema(schema: &acteon_core::Schema) -> Result<Self, String> {
        schema.validate().map_err(|e| e.to_string())?;
        let contract = Self {
            namespace: schema.namespace.clone(),
            tenant: schema.tenant.clone(),
            subject: schema.subject.clone(),
            version: schema.version,
            sha256: digest(&schema.body),
            body: schema.body.clone(),
        };
        contract.compile()?;
        Ok(contract)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        acteon_core::Schema::new(
            &self.subject,
            self.version,
            &self.namespace,
            &self.tenant,
            self.body.clone(),
        )
        .validate()
        .map_err(|e| e.to_string())?;
        if serde_json::to_vec(&self.body)
            .map_err(|e| e.to_string())?
            .len()
            > 256 * 1024
            || self.sha256 != digest(&self.body)
        {
            return Err("consume schema exceeds size limit or digest does not match".into());
        }
        Ok(())
    }
    pub(crate) fn compile(&self) -> Result<jsonschema::Validator, String> {
        self.validate()?;
        jsonschema::options()
            .with_retriever(NoExternalReferences)
            .should_validate_formats(true)
            .build(&self.body)
            .map_err(|e| e.to_string())
    }
}
struct NoExternalReferences;
impl jsonschema::Retrieve for NoExternalReferences {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("consume contracts must be self-contained; external references are disabled".into())
    }
}
pub(crate) fn digest(body: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted = map
                    .iter()
                    .map(|(k, v)| (k.clone(), canonical(v)))
                    .collect::<BTreeMap<_, _>>();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(body)).expect("JSON value serializes"))
    )
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamPoisonPolicy {
    #[default]
    Halt,
    Quarantine,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamInputPolicy {
    /// If nonempty, every received logical source must have a pinned contract.
    pub contracts: BTreeMap<String, StreamInputContract>,
    pub poison_policy: StreamPoisonPolicy,
    pub max_quarantined_records: usize,
    /// Bounds the full serialized retained entries, including transport envelopes.
    pub max_quarantine_bytes: usize,
}
impl Default for StreamInputPolicy {
    fn default() -> Self {
        Self {
            contracts: BTreeMap::new(),
            poison_policy: StreamPoisonPolicy::Halt,
            max_quarantined_records: 1000,
            max_quarantine_bytes: 16 * 1024 * 1024,
        }
    }
}
impl StreamInputPolicy {
    pub(crate) fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.contracts.len() > 64
            || self.max_quarantined_records == 0
            || self.max_quarantine_bytes == 0
        {
            return Err(
                "consume policy requires positive quarantine bounds and at most 64 contracts"
                    .into(),
            );
        }
        for (source, contract) in &self.contracts {
            if source.trim().is_empty() || source.len() > 256 {
                return Err("invalid contract source name".into());
            }
            contract.validate()?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamInputFailure {
    SchemaViolation,
    TypedDecode,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamQuarantinedInput {
    pub id: String,
    pub position: StreamPosition,
    /// Full original envelope. Receipt capabilities are deliberately not retained.
    pub message: BusMessage,
    pub failed_at: DateTime<Utc>,
    pub failure: StreamInputFailure,
    pub contract_sha256: Option<String>,
    /// Bounded, payload-free diagnostic.
    pub reason: String,
}
