use serde::{Deserialize, Serialize};

/// A responder (team, user, schedule, or escalation) that should
/// receive the alert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsGenieResponder {
    /// Responder name (for `team` / `user` / `schedule` / `escalation`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Responder UUID (alternative to `name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Responder user name (alternative to `name` / `id` for user responders).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Responder type: `"team"`, `"user"`, `"schedule"`, or `"escalation"`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// Request body for the Alert API v2 `POST /v2/alerts` endpoint.
///
/// Only the `message` field is actually required by the API. Everything
/// else is optional so operators can start with the minimum and layer in
/// tags/responders/details as their runbook evolves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsGenieAlertRequest {
    /// Short alert message (max 130 characters per the API).
    pub message: String,
    /// Client-side deduplication alias. Alerts sharing an alias
    /// collapse into a single incident so `trigger`/`ack`/`close`
    /// event sequences map onto the same incident lifecycle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Long-form alert description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Responders that should receive the alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responders: Option<Vec<OpsGenieResponder>>,
    /// Visibility list (teams or users allowed to see the alert).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_to: Option<Vec<OpsGenieResponder>>,
    /// Pre-defined actions that can be executed against the alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
    /// Free-form tags used for downstream routing / grouping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Arbitrary key-value details (shown in the alert UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Domain entity the alert is about (e.g. `"web-01"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// Source label shown on the alert (e.g. `"prometheus"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Alert priority (`P1`..=`P5`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// Username to attribute the alert to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Operator note attached to the alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Request body for the `acknowledge` and `close` lifecycle endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpsGenieLifecycleRequest {
    /// Source label (e.g. `"acteon"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Username performing the action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Operator note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Response body returned by the Alert API v2 endpoints.
///
/// `OpsGenie` alert creation is asynchronous: the server enqueues the
/// request and returns 202 with a `requestId` that can be used to
/// query the final state via the request-status endpoint. For our
/// fire-and-forget semantics we treat 202 as success and surface the
/// `request_id` in the action outcome body so operators can correlate
/// it with the alert that eventually appears in the `OpsGenie` UI.
///
/// Deserialization uses camelCase because `OpsGenie`'s API (and most
/// of its SDKs) serialize fields that way.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpsGenieApiResponse {
    /// Human-readable status string (e.g. `"Request will be processed"`).
    #[serde(default)]
    pub result: String,
    /// Duration the server took to handle the request, in seconds.
    #[serde(default)]
    pub took: f64,
    /// Request ID used to poll the request-status endpoint.
    #[serde(default)]
    pub request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_request_serializes_with_minimum_fields() {
        let req = OpsGenieAlertRequest {
            message: "High CPU".into(),
            alias: None,
            description: None,
            responders: None,
            visible_to: None,
            actions: None,
            tags: None,
            details: None,
            entity: None,
            source: None,
            priority: None,
            user: None,
            note: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["message"], "High CPU");
        assert!(json.get("alias").is_none());
        assert!(json.get("description").is_none());
        assert!(json.get("responders").is_none());
    }

    #[test]
    fn alert_request_serializes_full() {
        let req = OpsGenieAlertRequest {
            message: "Alert".into(),
            alias: Some("web-01-high-cpu".into()),
            description: Some("detailed".into()),
            responders: Some(vec![OpsGenieResponder {
                name: Some("ops-team".into()),
                id: None,
                username: None,
                kind: "team".into(),
            }]),
            visible_to: None,
            actions: Some(vec!["ping".into(), "reboot".into()]),
            tags: Some(vec!["critical".into(), "cpu".into()]),
            details: Some(serde_json::json!({"region": "us-east-1"})),
            entity: Some("web-01".into()),
            source: Some("acteon".into()),
            priority: Some("P1".into()),
            user: Some("acteon".into()),
            note: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["alias"], "web-01-high-cpu");
        assert_eq!(json["priority"], "P1");
        assert_eq!(json["tags"][0], "critical");
        assert_eq!(json["responders"][0]["name"], "ops-team");
        assert_eq!(json["responders"][0]["type"], "team");
        assert_eq!(json["details"]["region"], "us-east-1");
        assert!(json.get("note").is_none(), "note omitted when None");
    }

    #[test]
    fn responder_serializes_kind_as_type() {
        let responder = OpsGenieResponder {
            name: Some("alice".into()),
            id: None,
            username: None,
            kind: "user".into(),
        };
        let json = serde_json::to_value(&responder).unwrap();
        assert_eq!(json["type"], "user");
        assert!(json.get("kind").is_none());
    }

    #[test]
    fn lifecycle_request_serializes_empty() {
        let req = OpsGenieLifecycleRequest::default();
        let json = serde_json::to_value(&req).unwrap();
        assert!(
            json.as_object().unwrap().is_empty(),
            "default lifecycle request serializes to {{}}"
        );
    }

    #[test]
    fn lifecycle_request_serializes_with_note() {
        let req = OpsGenieLifecycleRequest {
            source: Some("acteon".into()),
            user: Some("runbook".into()),
            note: Some("auto-acknowledged by scheduled workflow".into()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["source"], "acteon");
        assert_eq!(json["user"], "runbook");
        assert_eq!(json["note"], "auto-acknowledged by scheduled workflow");
    }

    #[test]
    fn api_response_deserializes() {
        let json = r#"{"result":"Request will be processed","took":0.302,"requestId":"abc-123"}"#;
        let resp: OpsGenieApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.result, "Request will be processed");
        assert!((resp.took - 0.302).abs() < 1e-6);
        assert_eq!(resp.request_id, "abc-123");
    }

    #[test]
    fn api_response_tolerates_missing_fields() {
        // OpsGenie sometimes returns a sparser body for error/edge cases.
        let resp: OpsGenieApiResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(resp.result, "");
        assert_eq!(resp.request_id, "");
    }
}
