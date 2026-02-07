use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Response received from the webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookResponse {
    /// HTTP status code from the endpoint.
    pub status_code: u16,

    /// Response body (parsed as JSON if possible, otherwise a string value).
    pub body: serde_json::Value,

    /// Response headers.
    pub headers: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_response_serializes() {
        let resp = WebhookResponse {
            status_code: 200,
            body: serde_json::json!({"ok": true}),
            headers: HashMap::new(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status_code"], 200);
        assert_eq!(json["body"]["ok"], true);
    }

    #[test]
    fn webhook_response_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-Request-Id".into(), "abc-123".into());

        let resp = WebhookResponse {
            status_code: 201,
            body: serde_json::json!(null),
            headers,
        };

        assert_eq!(resp.headers.get("X-Request-Id").unwrap(), "abc-123");
    }

    #[test]
    fn webhook_response_serde_roundtrip() {
        let resp = WebhookResponse {
            status_code: 202,
            body: serde_json::json!({"id": "event-1"}),
            headers: HashMap::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: WebhookResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status_code, 202);
        assert_eq!(back.body["id"], "event-1");
    }
}
