//! Bus payload schema — JSON Schema registry entry for a bus topic.
//!
//! Phase 3 of the agentic bus adds publish-edge validation. Schemas are
//! registered as immutable `(subject, version)` pairs — posting to an
//! existing subject allocates a new version, never replaces the old
//! one. Topics opt into validation by binding to a subject + version
//! via `Topic::schema_subject` / `Topic::schema_version`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Wire format a schema document is written in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SchemaFormat {
    /// JSON Schema (draft 2020-12 in practice — `jsonschema` crate
    /// default). The only format in V1; Avro/Protobuf come later.
    #[default]
    JsonSchema,
}

/// A schema definition tied to a `(namespace, tenant, subject, version)`
/// tuple.
///
/// `subject` is the logical name operators use ("order-v1"); `version`
/// is a monotonic integer assigned by the server starting at 1.
/// Schemas are immutable once registered; updating "the schema" means
/// publishing a new version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Schema {
    /// Logical schema name (e.g. `"orders"`). Namespaced per tenant.
    pub subject: String,
    /// Monotonic version, starting at 1. Assigned by the server on
    /// registration.
    pub version: i32,
    /// Namespace the schema lives in.
    pub namespace: String,
    /// Tenant that owns the schema.
    pub tenant: String,
    /// Wire format the `body` is written in.
    #[serde(default)]
    pub format: SchemaFormat,
    /// The schema document itself (JSON Schema in V1). Held as `Value`
    /// so callers don't have to re-parse; the bus-side validator
    /// compiles it once into a `jsonschema::Validator`.
    pub body: serde_json::Value,
    /// Arbitrary operator labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// When this version was registered.
    pub created_at: DateTime<Utc>,
}

impl Schema {
    /// Construct a schema with an explicit version. Callers generally
    /// use the server endpoint; this constructor is primarily for tests
    /// and for loading from state.
    #[must_use]
    pub fn new(
        subject: impl Into<String>,
        version: i32,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        body: serde_json::Value,
    ) -> Self {
        Self {
            subject: subject.into(),
            version,
            namespace: namespace.into(),
            tenant: tenant.into(),
            format: SchemaFormat::default(),
            body,
            labels: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Stable ID used as the state-store key — `"{subject}:{version}"`.
    /// Combined with `(namespace, tenant)` in `KeyKind::BusSchema` this
    /// gives O(1) lookup without a scan. (`KeyKind` lives in
    /// `acteon-state`, so we use a plain code span here to avoid a
    /// cross-crate rustdoc intra-doc link.)
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.subject, self.version)
    }

    /// Validate the schema's identity fields (subject + version +
    /// namespace + tenant). The body itself is validated at the bus
    /// layer when it's compiled into a validator.
    pub fn validate(&self) -> Result<(), SchemaValidationError> {
        Self::validate_fragment(&self.namespace)?;
        Self::validate_fragment(&self.tenant)?;
        Self::validate_subject(&self.subject)?;
        if self.version < 1 {
            return Err(SchemaValidationError::InvalidVersion(self.version));
        }
        Ok(())
    }

    /// Subject validation — same character set as topic fragments but a
    /// larger length budget since subjects can be more descriptive
    /// (`"order-created-v2"`).
    pub fn validate_subject(s: &str) -> Result<(), SchemaValidationError> {
        if s.is_empty() {
            return Err(SchemaValidationError::EmptySubject);
        }
        if s.len() > 120 {
            return Err(SchemaValidationError::SubjectTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(SchemaValidationError::InvalidSubjectChar(s.to_string()));
        }
        Ok(())
    }

    /// Namespace / tenant validation — reuses the topic fragment rules
    /// so a schema subject is addressable under the same key shape as
    /// its bound topic.
    pub fn validate_fragment(s: &str) -> Result<(), SchemaValidationError> {
        if s.is_empty() {
            return Err(SchemaValidationError::EmptyFragment);
        }
        if s.len() > 80 {
            return Err(SchemaValidationError::FragmentTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SchemaValidationError::InvalidFragmentChar(s.to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchemaValidationError {
    #[error("schema subject must not be empty")]
    EmptySubject,
    #[error("schema subject exceeds 120 characters")]
    SubjectTooLong,
    #[error("schema subject '{0}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidSubjectChar(String),
    #[error("schema namespace/tenant must not be empty")]
    EmptyFragment,
    #[error("schema namespace/tenant fragment exceeds 80 characters")]
    FragmentTooLong,
    #[error("schema fragment '{0}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidFragmentChar(String),
    #[error("schema version must be >= 1 (got {0})")]
    InvalidVersion(i32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn id_is_subject_colon_version() {
        let s = Schema::new("orders", 3, "agents", "demo", json!({"type": "object"}));
        assert_eq!(s.id(), "orders:3");
    }

    #[test]
    fn validate_rejects_zero_version() {
        let s = Schema::new("orders", 0, "agents", "demo", json!({}));
        assert_eq!(s.validate(), Err(SchemaValidationError::InvalidVersion(0)));
    }

    #[test]
    fn validate_rejects_empty_subject() {
        let s = Schema::new("", 1, "agents", "demo", json!({}));
        assert_eq!(s.validate(), Err(SchemaValidationError::EmptySubject));
    }

    #[test]
    fn validate_rejects_subject_with_slash() {
        let s = Schema::new("orders/v1", 1, "agents", "demo", json!({}));
        assert!(matches!(
            s.validate(),
            Err(SchemaValidationError::InvalidSubjectChar(_))
        ));
    }

    #[test]
    fn validate_accepts_dotted_subject() {
        // Dots are allowed in subjects (unlike topic fragments) because
        // subjects are opaque strings and versioned naming like
        // "orders.v1" is conventional.
        let s = Schema::new("orders.v1", 1, "agents", "demo", json!({}));
        s.validate().unwrap();
    }

    #[test]
    fn roundtrip_serde() {
        let mut s = Schema::new("orders", 2, "agents", "demo", json!({"type": "object"}));
        s.labels.insert("owner".to_string(), "payments".to_string());
        let j = serde_json::to_string(&s).unwrap();
        let back: Schema = serde_json::from_str(&j).unwrap();
        assert_eq!(back.subject, s.subject);
        assert_eq!(back.version, s.version);
        assert_eq!(back.labels.get("owner"), Some(&"payments".to_string()));
    }
}
