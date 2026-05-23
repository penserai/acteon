//! Field redaction for audit records.
//!
//! This module provides a [`RedactingAuditStore`] wrapper that redacts sensitive
//! fields from action payloads before storing audit records.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::analytics::AnalyticsStore;
use crate::error::AuditError;
use crate::record::{AuditPage, AuditQuery, AuditRecord};
use crate::store::AuditStore;

/// Configuration for field redaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedactConfig {
    /// List of field names or paths to redact (case-insensitive).
    ///
    /// Supports nested paths using dot notation (e.g., `"credentials.password"`).
    /// Field matching is case-insensitive.
    #[serde(default)]
    pub fields: Vec<String>,

    /// Placeholder text to replace redacted values with.
    #[serde(default = "default_placeholder")]
    pub placeholder: String,
}

fn default_placeholder() -> String {
    "[REDACTED]".to_owned()
}

impl RedactConfig {
    /// Create a new `RedactConfig` with the given fields to redact.
    pub fn new(fields: Vec<String>) -> Self {
        Self {
            fields,
            placeholder: default_placeholder(),
        }
    }

    /// Set a custom placeholder for redacted values.
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
}

/// Performs field redaction on JSON values.
#[derive(Debug, Clone)]
pub struct Redactor {
    /// Lowercased field names/paths to match.
    fields: Vec<String>,
    /// Replacement value for redacted fields.
    placeholder: serde_json::Value,
}

impl Redactor {
    /// Create a new `Redactor` from the given configuration.
    pub fn new(config: &RedactConfig) -> Self {
        Self {
            fields: config.fields.iter().map(|f| f.to_lowercase()).collect(),
            placeholder: serde_json::Value::String(config.placeholder.clone()),
        }
    }

    /// Redact sensitive fields from the given JSON value in place.
    ///
    /// This method recursively traverses the JSON structure and replaces
    /// values for matching field names with the placeholder.
    pub fn redact(&self, value: &mut serde_json::Value) {
        self.redact_at_path(value, &[]);
    }

    /// Recursively redact fields, tracking the current path.
    fn redact_at_path(&self, value: &mut serde_json::Value, path: &[&str]) {
        match value {
            serde_json::Value::Object(map) => {
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    let key_lower = key.to_lowercase();

                    // Build the full path for this key.
                    let mut full_path = path.to_vec();
                    full_path.push(&key_lower);
                    let full_path_str = full_path.join(".");

                    // Check if this field (by name alone or full path) should be redacted.
                    let should_redact = self.fields.iter().any(|f| {
                        // Match exact field name (case-insensitive).
                        *f == key_lower ||
                        // Match full path (case-insensitive).
                        *f == full_path_str
                    });

                    if should_redact {
                        if let Some(v) = map.get_mut(&key) {
                            *v = self.placeholder.clone();
                        }
                    } else if let Some(v) = map.get_mut(&key) {
                        // Recurse into nested objects/arrays.
                        // We need to convert full_path to owned strings for the recursive call.
                        let owned_path: Vec<String> =
                            full_path.iter().map(|s| (*s).to_owned()).collect();
                        let path_refs: Vec<&str> = owned_path.iter().map(String::as_str).collect();
                        self.redact_at_path(v, &path_refs);
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                // Recurse into array elements with the same path.
                for item in arr {
                    self.redact_at_path(item, path);
                }
            }
            _ => {
                // Primitive values at the top level are not redacted.
            }
        }
    }
}

/// An audit store wrapper that redacts sensitive fields before storage.
///
/// This wrapper implements the decorator pattern, wrapping any [`AuditStore`]
/// implementation and applying field redaction to the `action_payload` before
/// delegating to the inner store.
pub struct RedactingAuditStore {
    inner: Arc<dyn AuditStore>,
    redactor: Redactor,
}

impl RedactingAuditStore {
    /// Create a new `RedactingAuditStore` wrapping the given inner store.
    pub fn new(inner: Arc<dyn AuditStore>, config: &RedactConfig) -> Self {
        Self {
            inner,
            redactor: Redactor::new(config),
        }
    }
}

#[async_trait]
impl AuditStore for RedactingAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let mut redacted = entry;
        if let Some(ref mut payload) = redacted.action_payload {
            self.redactor.redact(payload);
        }
        self.inner.record(redacted).await
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        self.inner.get_by_action_id(action_id).await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        self.inner.get_by_id(id).await
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        self.inner.query(query).await
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        self.inner.cleanup_expired().await
    }

    fn analytics(&self) -> Option<Arc<dyn AnalyticsStore>> {
        self.inner.analytics()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_redactor() -> Redactor {
        Redactor::new(&RedactConfig::new(vec![
            "password".to_owned(),
            "token".to_owned(),
            "api_key".to_owned(),
            "secret".to_owned(),
            "ssn".to_owned(),
            "credentials.password".to_owned(),
        ]))
    }

    #[test]
    fn redact_simple_fields() {
        let mut value = json!({
            "username": "alice",
            "password": "secret123",
            "email": "alice@example.com"
        });

        test_redactor().redact(&mut value);

        assert_eq!(value["username"], "alice");
        assert_eq!(value["password"], "[REDACTED]");
        assert_eq!(value["email"], "alice@example.com");
    }

    #[test]
    fn redact_case_insensitive() {
        let mut value = json!({
            "PASSWORD": "secret123",
            "Token": "abc123",
            "API_KEY": "key456"
        });

        test_redactor().redact(&mut value);

        assert_eq!(value["PASSWORD"], "[REDACTED]");
        assert_eq!(value["Token"], "[REDACTED]");
        assert_eq!(value["API_KEY"], "[REDACTED]");
    }

    #[test]
    fn redact_nested_by_field_name() {
        let mut value = json!({
            "user": {
                "name": "alice",
                "password": "secret123"
            }
        });

        test_redactor().redact(&mut value);

        assert_eq!(value["user"]["name"], "alice");
        assert_eq!(value["user"]["password"], "[REDACTED]");
    }

    #[test]
    fn redact_nested_by_path() {
        let mut value = json!({
            "credentials": {
                "username": "alice",
                "password": "secret123"
            },
            "other": {
                "data": "visible"
            }
        });

        // The path "credentials.password" should match.
        test_redactor().redact(&mut value);

        assert_eq!(value["credentials"]["username"], "alice");
        assert_eq!(value["credentials"]["password"], "[REDACTED]");
        assert_eq!(value["other"]["data"], "visible");
    }

    #[test]
    fn redact_arrays_of_objects() {
        let mut value = json!({
            "users": [
                { "name": "alice", "password": "pass1" },
                { "name": "bob", "password": "pass2" }
            ]
        });

        test_redactor().redact(&mut value);

        assert_eq!(value["users"][0]["name"], "alice");
        assert_eq!(value["users"][0]["password"], "[REDACTED]");
        assert_eq!(value["users"][1]["name"], "bob");
        assert_eq!(value["users"][1]["password"], "[REDACTED]");
    }

    #[test]
    fn redact_deeply_nested() {
        let mut value = json!({
            "level1": {
                "level2": {
                    "level3": {
                        "secret": "hidden",
                        "visible": "shown"
                    }
                }
            }
        });

        test_redactor().redact(&mut value);

        assert_eq!(value["level1"]["level2"]["level3"]["secret"], "[REDACTED]");
        assert_eq!(value["level1"]["level2"]["level3"]["visible"], "shown");
    }

    #[test]
    fn redact_with_custom_placeholder() {
        let config = RedactConfig::new(vec!["password".to_owned()]).with_placeholder("***");
        let redactor = Redactor::new(&config);

        let mut value = json!({ "password": "secret123" });
        redactor.redact(&mut value);

        assert_eq!(value["password"], "***");
    }

    #[test]
    fn redact_preserves_structure() {
        let mut value = json!({
            "config": {
                "api_key": "key123",
                "settings": {
                    "debug": true,
                    "token": "tok456"
                }
            }
        });

        test_redactor().redact(&mut value);

        assert_eq!(value["config"]["api_key"], "[REDACTED]");
        assert_eq!(value["config"]["settings"]["debug"], true);
        assert_eq!(value["config"]["settings"]["token"], "[REDACTED]");
    }

    #[test]
    fn redact_no_matching_fields() {
        let mut value = json!({
            "username": "alice",
            "email": "alice@example.com"
        });
        let original = value.clone();

        test_redactor().redact(&mut value);

        assert_eq!(value, original);
    }

    #[test]
    fn redact_empty_object() {
        let mut value = json!({});
        test_redactor().redact(&mut value);
        assert_eq!(value, json!({}));
    }

    #[test]
    fn redact_primitives_unchanged() {
        let mut value = json!("just a string");
        test_redactor().redact(&mut value);
        assert_eq!(value, json!("just a string"));

        let mut value = json!(42);
        test_redactor().redact(&mut value);
        assert_eq!(value, json!(42));
    }
}
