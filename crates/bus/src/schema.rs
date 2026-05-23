//! Payload schema validation (Phase 3).
//!
//! The bus publish edge validates payloads against a JSON Schema that a
//! topic is bound to. Schemas are compiled once into a
//! [`jsonschema::Validator`] and cached keyed by `(namespace, tenant,
//! subject, version)`. Cache entries are invalidated when a schema is
//! deleted or a topic's binding changes.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

/// Compiled-schema registry. Cheap to clone; the underlying map is
/// shared and mutated under a write lock only on register / invalidate.
#[derive(Clone, Default)]
pub struct SchemaValidator {
    inner: Arc<RwLock<HashMap<CacheKey, Arc<jsonschema::Validator>>>>,
}

/// Composite key for the compiled-validator cache.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    namespace: String,
    tenant: String,
    subject: String,
    version: i32,
}

impl SchemaValidator {
    /// Fresh, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile `body` and insert the resulting validator under
    /// `(namespace, tenant, subject, version)`. Returns an error if the
    /// schema body isn't a valid JSON Schema.
    ///
    /// Replacing an existing entry is permitted (same tuple → recompile
    /// after edit). Callers should invalidate when a subject's version
    /// is deleted via [`Self::remove`].
    pub fn register(
        &self,
        namespace: &str,
        tenant: &str,
        subject: &str,
        version: i32,
        body: &serde_json::Value,
    ) -> Result<(), SchemaValidatorError> {
        let validator = jsonschema::validator_for(body)
            .map_err(|e| SchemaValidatorError::CompileFailed(e.to_string()))?;
        let key = CacheKey {
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            subject: subject.to_string(),
            version,
        };
        self.inner.write().insert(key, Arc::new(validator));
        Ok(())
    }

    /// Validate `payload` against the cached validator for `(namespace,
    /// tenant, subject, version)`. Returns
    /// [`SchemaValidatorError::NotFound`] if no such validator exists —
    /// callers are expected to have registered before dispatch.
    pub fn validate(
        &self,
        namespace: &str,
        tenant: &str,
        subject: &str,
        version: i32,
        payload: &serde_json::Value,
    ) -> Result<(), SchemaValidatorError> {
        let key = CacheKey {
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            subject: subject.to_string(),
            version,
        };
        let validator = {
            let guard = self.inner.read();
            guard.get(&key).cloned()
        };
        let Some(v) = validator else {
            return Err(SchemaValidatorError::NotFound {
                subject: subject.to_string(),
                version,
            });
        };
        let errors: Vec<_> = v
            .iter_errors(payload)
            .take(10)
            .map(|e| ValidationIssue {
                path: e.instance_path().to_string(),
                message: e.to_string(),
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(SchemaValidatorError::InvalidPayload(errors))
        }
    }

    /// Drop any cached validator for the given tuple. No-op if absent.
    pub fn remove(&self, namespace: &str, tenant: &str, subject: &str, version: i32) {
        let key = CacheKey {
            namespace: namespace.to_string(),
            tenant: tenant.to_string(),
            subject: subject.to_string(),
            version,
        };
        self.inner.write().remove(&key);
    }

    /// Test helper: number of cached validators.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }
}

/// Detail of a single JSON-Schema violation, surfaced to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// JSON Pointer into the payload where the error occurred
    /// (e.g. `"/order/items/0/qty"`).
    pub path: String,
    /// Human-readable message from the `jsonschema` crate.
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaValidatorError {
    #[error("schema body failed to compile: {0}")]
    CompileFailed(String),
    #[error("no validator cached for subject '{subject}' version {version}")]
    NotFound { subject: String, version: i32 },
    #[error("payload failed schema validation: {} issue(s)", .0.len())]
    InvalidPayload(Vec<ValidationIssue>),
}

impl SchemaValidatorError {
    /// The list of validation issues if this is an
    /// [`Self::InvalidPayload`] error — useful for surfacing details
    /// over HTTP without matching manually.
    #[must_use]
    pub fn issues(&self) -> Option<&[ValidationIssue]> {
        match self {
            Self::InvalidPayload(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn order_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["id", "qty"],
            "properties": {
                "id": {"type": "string"},
                "qty": {"type": "integer", "minimum": 1}
            }
        })
    }

    #[test]
    fn register_and_validate_accepts_good_payload() {
        let v = SchemaValidator::new();
        v.register("ns", "t", "orders", 1, &order_schema()).unwrap();
        v.validate("ns", "t", "orders", 1, &json!({"id": "a", "qty": 2}))
            .unwrap();
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let v = SchemaValidator::new();
        v.register("ns", "t", "orders", 1, &order_schema()).unwrap();
        let err = v
            .validate("ns", "t", "orders", 1, &json!({"id": "a"}))
            .unwrap_err();
        let issues = err.issues().expect("issues");
        assert!(!issues.is_empty());
    }

    #[test]
    fn validate_rejects_wrong_type() {
        let v = SchemaValidator::new();
        v.register("ns", "t", "orders", 1, &order_schema()).unwrap();
        let err = v
            .validate("ns", "t", "orders", 1, &json!({"id": "a", "qty": "nope"}))
            .unwrap_err();
        let issues = err.issues().expect("issues");
        assert!(issues.iter().any(|i| i.path.contains("qty")));
    }

    #[test]
    fn validate_not_found_for_unknown_subject() {
        let v = SchemaValidator::new();
        let err = v.validate("ns", "t", "missing", 1, &json!({})).unwrap_err();
        assert!(matches!(err, SchemaValidatorError::NotFound { .. }));
    }

    #[test]
    fn register_rejects_bad_schema_body() {
        let v = SchemaValidator::new();
        // "type" must be a string or array — integer is invalid.
        let bad = json!({"type": 42});
        let err = v.register("ns", "t", "x", 1, &bad).unwrap_err();
        assert!(matches!(err, SchemaValidatorError::CompileFailed(_)));
    }

    #[test]
    fn remove_drops_cached_validator() {
        let v = SchemaValidator::new();
        v.register("ns", "t", "orders", 1, &order_schema()).unwrap();
        assert_eq!(v.len(), 1);
        v.remove("ns", "t", "orders", 1);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn versions_are_independent() {
        let v = SchemaValidator::new();
        v.register("ns", "t", "orders", 1, &order_schema()).unwrap();
        // v2 requires an extra field.
        let stricter = json!({
            "type": "object",
            "required": ["id", "qty", "sku"],
            "properties": {
                "id": {"type": "string"},
                "qty": {"type": "integer"},
                "sku": {"type": "string"}
            }
        });
        v.register("ns", "t", "orders", 2, &stricter).unwrap();
        // v1 accepts the old shape…
        v.validate("ns", "t", "orders", 1, &json!({"id": "a", "qty": 1}))
            .unwrap();
        // …v2 rejects it.
        assert!(
            v.validate("ns", "t", "orders", 2, &json!({"id": "a", "qty": 1}))
                .is_err()
        );
    }
}
