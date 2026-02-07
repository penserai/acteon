//! Convenience helpers for creating webhook-targeted actions.
//!
//! # Example
//!
//! ```no_run
//! use acteon_client::webhook;
//!
//! let action = webhook::action("notifications", "tenant-1")
//!     .url("https://hooks.example.com/alert")
//!     .body(serde_json::json!({"message": "Server is down", "severity": "critical"}))
//!     .header("X-Custom-Header", "value")
//!     .build();
//! ```

use acteon_core::Action;
use std::collections::HashMap;

/// Create a new webhook action builder.
///
/// This is a convenience entry point for building actions targeted at the
/// webhook provider.
pub fn action(namespace: impl Into<String>, tenant: impl Into<String>) -> WebhookActionBuilder {
    WebhookActionBuilder {
        namespace: namespace.into(),
        tenant: tenant.into(),
        url: String::new(),
        method: "POST".to_string(),
        action_type: "webhook".to_string(),
        body: serde_json::Value::Object(serde_json::Map::new()),
        headers: HashMap::new(),
        dedup_key: None,
    }
}

/// Builder for constructing webhook-targeted Actions.
#[derive(Debug)]
pub struct WebhookActionBuilder {
    namespace: String,
    tenant: String,
    url: String,
    method: String,
    action_type: String,
    body: serde_json::Value,
    headers: HashMap<String, String>,
    dedup_key: Option<String>,
}

impl WebhookActionBuilder {
    /// Set the target URL for the webhook.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Set the HTTP method (default: "POST").
    #[must_use]
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// Set the action type (default: "webhook").
    #[must_use]
    pub fn action_type(mut self, action_type: impl Into<String>) -> Self {
        self.action_type = action_type.into();
        self
    }

    /// Set the JSON body to send to the webhook endpoint.
    #[must_use]
    pub fn body(mut self, body: serde_json::Value) -> Self {
        self.body = body;
        self
    }

    /// Add a custom HTTP header to the webhook request.
    #[must_use]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set a deduplication key.
    #[must_use]
    pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
        self.dedup_key = Some(key.into());
        self
    }

    /// Build the Action.
    ///
    /// # Panics
    ///
    /// Panics if `url` has not been set.
    pub fn build(self) -> Action {
        assert!(!self.url.is_empty(), "webhook URL must be set");

        let mut payload = serde_json::Map::new();
        payload.insert("url".to_string(), serde_json::Value::String(self.url));
        payload.insert("method".to_string(), serde_json::Value::String(self.method));
        payload.insert("body".to_string(), self.body);

        if !self.headers.is_empty() {
            let headers_value = serde_json::to_value(&self.headers)
                .expect("HashMap<String, String> serialization cannot fail");
            payload.insert("headers".to_string(), headers_value);
        }

        let mut action = Action::new(
            self.namespace,
            self.tenant,
            "webhook",
            self.action_type,
            serde_json::Value::Object(payload),
        );

        if let Some(key) = self.dedup_key {
            action.dedup_key = Some(key);
        }

        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_webhook_action() {
        let a = action("ns", "t1")
            .url("https://example.com/hook")
            .body(serde_json::json!({"msg": "hello"}))
            .build();

        assert_eq!(a.namespace.as_str(), "ns");
        assert_eq!(a.tenant.as_str(), "t1");
        assert_eq!(a.provider.as_str(), "webhook");
        assert_eq!(a.action_type, "webhook");
        assert_eq!(a.payload["url"], "https://example.com/hook");
        assert_eq!(a.payload["method"], "POST");
        assert_eq!(a.payload["body"]["msg"], "hello");
        assert!(a.payload.get("headers").is_none());
    }

    #[test]
    fn webhook_action_with_options() {
        let a = action("ns", "t1")
            .url("https://example.com/hook")
            .method("PUT")
            .action_type("custom_hook")
            .body(serde_json::json!({"key": "value"}))
            .header("X-Custom", "abc")
            .header("Authorization", "Bearer tok")
            .dedup_key("dedup-1")
            .build();

        assert_eq!(a.action_type, "custom_hook");
        assert_eq!(a.payload["method"], "PUT");
        assert_eq!(a.payload["headers"]["X-Custom"], "abc");
        assert_eq!(a.payload["headers"]["Authorization"], "Bearer tok");
        assert_eq!(a.dedup_key.as_deref(), Some("dedup-1"));
    }

    #[test]
    #[should_panic(expected = "webhook URL must be set")]
    fn panics_without_url() {
        action("ns", "t1").body(serde_json::json!({})).build();
    }
}
