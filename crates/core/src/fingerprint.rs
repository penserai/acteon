//! Fingerprint computation for event correlation.
//!
//! Fingerprints are used to correlate related events across time,
//! enabling state machine tracking and event grouping.

use sha2::{Digest, Sha256};

use crate::Action;

/// Compute a fingerprint for an action based on specified fields.
///
/// The fingerprint is a hex-encoded SHA-256 hash of the concatenated
/// field values, providing a stable identifier for correlating related events.
///
/// # Arguments
///
/// * `action` - The action to compute fingerprint for
/// * `fields` - Field paths to include in the fingerprint (e.g., `["action_type", "metadata.cluster"]`)
///
/// # Returns
///
/// A hex-encoded fingerprint string.
///
/// # Example
///
/// ```
/// use acteon_core::{Action, fingerprint::compute_fingerprint};
///
/// let action = Action::new("ns", "tenant", "provider", "alert", serde_json::json!({}));
/// let fp = compute_fingerprint(&action, &["action_type".to_string(), "tenant".to_string()]);
/// assert!(!fp.is_empty());
/// ```
#[must_use]
pub fn compute_fingerprint(action: &Action, fields: &[String]) -> String {
    let mut hasher = Sha256::new();

    for field in fields {
        let value = extract_field_value(action, field);
        hasher.update(field.as_bytes());
        hasher.update(b"=");
        hasher.update(value.as_bytes());
        hasher.update(b";");
    }

    let result = hasher.finalize();
    hex::encode(result)
}

/// Extract a field value from an action by path.
///
/// Supports the following field paths:
/// - `namespace`, `tenant`, `provider`, `action_type`, `id`
/// - `metadata.<key>` - for metadata labels
/// - `payload.<path>` - for JSON payload fields (supports nested paths with `.`)
fn extract_field_value(action: &Action, field: &str) -> String {
    match field {
        "namespace" => action.namespace.as_str().to_string(),
        "tenant" => action.tenant.as_str().to_string(),
        "provider" => action.provider.as_str().to_string(),
        "action_type" => action.action_type.clone(),
        "id" => action.id.as_str().to_string(),
        "status" => action.status.clone().unwrap_or_default(),
        path if path.starts_with("metadata.") => {
            let key = &path[9..]; // Skip "metadata."
            action.metadata.labels.get(key).cloned().unwrap_or_default()
        }
        path if path.starts_with("payload.") => {
            let json_path = &path[8..]; // Skip "payload."
            extract_json_value(&action.payload, json_path)
        }
        _ => String::new(),
    }
}

/// Extract a value from JSON by dot-separated path.
fn extract_json_value(value: &serde_json::Value, path: &str) -> String {
    let mut current = value;

    for part in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                current = match map.get(part) {
                    Some(v) => v,
                    None => return String::new(),
                };
            }
            serde_json::Value::Array(arr) => {
                if let Ok(idx) = part.parse::<usize>() {
                    current = match arr.get(idx) {
                        Some(v) => v,
                        None => return String::new(),
                    };
                } else {
                    return String::new();
                }
            }
            _ => return String::new(),
        }
    }

    match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        _ => current.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActionMetadata;
    use std::collections::HashMap;

    #[test]
    fn compute_fingerprint_basic_fields() {
        let action = Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_alert",
            serde_json::json!({}),
        );

        let fp = compute_fingerprint(&action, &["action_type".to_string(), "tenant".to_string()]);

        // Fingerprint should be deterministic
        let fp2 = compute_fingerprint(&action, &["action_type".to_string(), "tenant".to_string()]);
        assert_eq!(fp, fp2);

        // Different field order should produce different fingerprint
        let fp3 = compute_fingerprint(&action, &["tenant".to_string(), "action_type".to_string()]);
        assert_ne!(fp, fp3);
    }

    #[test]
    fn compute_fingerprint_with_metadata() {
        let mut labels = HashMap::new();
        labels.insert("cluster".to_string(), "prod-1".to_string());
        labels.insert("region".to_string(), "us-east".to_string());

        let action = Action::new("ns", "t", "p", "alert", serde_json::json!({}))
            .with_metadata(ActionMetadata { labels });

        let fp = compute_fingerprint(
            &action,
            &[
                "action_type".to_string(),
                "metadata.cluster".to_string(),
                "metadata.region".to_string(),
            ],
        );

        assert!(!fp.is_empty());
        assert_eq!(fp.len(), 64); // SHA-256 produces 64 hex chars
    }

    #[test]
    fn compute_fingerprint_with_payload() {
        let action = Action::new(
            "ns",
            "t",
            "p",
            "alert",
            serde_json::json!({
                "host": "server-1",
                "nested": {
                    "severity": "critical"
                }
            }),
        );

        let fp = compute_fingerprint(
            &action,
            &[
                "action_type".to_string(),
                "payload.host".to_string(),
                "payload.nested.severity".to_string(),
            ],
        );

        assert!(!fp.is_empty());
    }

    #[test]
    fn compute_fingerprint_missing_fields() {
        let action = Action::new("ns", "t", "p", "alert", serde_json::json!({}));

        // Missing fields should contribute empty string, not panic
        let fp = compute_fingerprint(
            &action,
            &[
                "action_type".to_string(),
                "metadata.nonexistent".to_string(),
                "payload.missing".to_string(),
            ],
        );

        assert!(!fp.is_empty());
    }

    #[test]
    fn extract_json_value_nested() {
        let json = serde_json::json!({
            "level1": {
                "level2": {
                    "value": "found"
                }
            }
        });

        assert_eq!(extract_json_value(&json, "level1.level2.value"), "found");
        assert_eq!(extract_json_value(&json, "level1.level2.missing"), "");
    }

    #[test]
    fn extract_json_value_array() {
        let json = serde_json::json!({
            "items": ["a", "b", "c"]
        });

        assert_eq!(extract_json_value(&json, "items.0"), "a");
        assert_eq!(extract_json_value(&json, "items.2"), "c");
        assert_eq!(extract_json_value(&json, "items.10"), "");
    }
}
