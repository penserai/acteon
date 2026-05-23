use serde::{Deserialize, Serialize};

/// `VictorOps` alert state. Maps one-to-one onto the strings
/// accepted by the `message_type` field of the REST endpoint
/// integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VictorOpsMessageType {
    /// Firing alert — pages the on-call.
    Critical,
    /// Lower-priority alert. Still visible but does not page.
    Warning,
    /// Informational alert. Appears in the timeline but does not page.
    Info,
    /// Oncall acknowledged the incident.
    Acknowledgement,
    /// Incident is resolved.
    Recovery,
}

impl VictorOpsMessageType {
    /// Parse an Acteon `event_action` string into the corresponding
    /// `VictorOps` message type.
    ///
    /// # Errors
    ///
    /// Returns `Err` with the unrecognized input wrapped in a
    /// static string when the action does not map to a known
    /// `VictorOps` state.
    pub fn parse(event_action: &str) -> Result<Self, String> {
        match event_action {
            "trigger" => Ok(Self::Critical),
            "warn" => Ok(Self::Warning),
            "info" => Ok(Self::Info),
            "acknowledge" => Ok(Self::Acknowledgement),
            "resolve" => Ok(Self::Recovery),
            other => Err(format!(
                "invalid event_action '{other}': must be one of 'trigger', 'warn', 'info', 'acknowledge', or 'resolve'"
            )),
        }
    }

    /// Return the wire-format string used in the `message_type`
    /// field of the JSON body.
    #[must_use]
    pub const fn as_wire(&self) -> &'static str {
        match self {
            Self::Critical => "CRITICAL",
            Self::Warning => "WARNING",
            Self::Info => "INFO",
            Self::Acknowledgement => "ACKNOWLEDGEMENT",
            Self::Recovery => "RECOVERY",
        }
    }
}

/// Request body for the `VictorOps` REST endpoint integration.
///
/// Only `message_type` is strictly required by the API; everything
/// else is optional so operators can start with the minimum and
/// layer in details as their runbook evolves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VictorOpsAlertRequest {
    /// Alert state (`CRITICAL`, `WARNING`, `INFO`,
    /// `ACKNOWLEDGEMENT`, or `RECOVERY`).
    pub message_type: VictorOpsMessageType,
    /// Client-side deduplication identifier. `VictorOps` correlates
    /// trigger / ack / resolve events that share an `entity_id`
    /// into a single incident.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// Human-readable short display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_display_name: Option<String>,
    /// Long-form alert body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_message: Option<String>,
    /// Value reported in the `monitoring_tool` field (defaults to
    /// the config's `monitoring_tool`, typically `"acteon"`).
    pub monitoring_tool: String,
    /// Optional host name the alert is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// Unix timestamp (seconds) of when the alerting condition started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_start_time: Option<i64>,
}

/// Response body returned by the `VictorOps` REST endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct VictorOpsApiResponse {
    /// Result status (`"success"` on happy path).
    #[serde(default)]
    pub result: String,
    /// Echoed `entity_id` from the request body.
    #[serde(default)]
    pub entity_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_type_parse_valid() {
        assert_eq!(
            VictorOpsMessageType::parse("trigger").unwrap(),
            VictorOpsMessageType::Critical
        );
        assert_eq!(
            VictorOpsMessageType::parse("warn").unwrap(),
            VictorOpsMessageType::Warning
        );
        assert_eq!(
            VictorOpsMessageType::parse("info").unwrap(),
            VictorOpsMessageType::Info
        );
        assert_eq!(
            VictorOpsMessageType::parse("acknowledge").unwrap(),
            VictorOpsMessageType::Acknowledgement
        );
        assert_eq!(
            VictorOpsMessageType::parse("resolve").unwrap(),
            VictorOpsMessageType::Recovery
        );
    }

    #[test]
    fn message_type_parse_invalid() {
        let err = VictorOpsMessageType::parse("snooze").unwrap_err();
        assert!(err.contains("snooze"));
    }

    #[test]
    fn message_type_wire_format() {
        assert_eq!(VictorOpsMessageType::Critical.as_wire(), "CRITICAL");
        assert_eq!(VictorOpsMessageType::Warning.as_wire(), "WARNING");
        assert_eq!(VictorOpsMessageType::Info.as_wire(), "INFO");
        assert_eq!(
            VictorOpsMessageType::Acknowledgement.as_wire(),
            "ACKNOWLEDGEMENT"
        );
        assert_eq!(VictorOpsMessageType::Recovery.as_wire(), "RECOVERY");
    }

    #[test]
    fn alert_request_serializes_minimum() {
        let req = VictorOpsAlertRequest {
            message_type: VictorOpsMessageType::Critical,
            entity_id: None,
            entity_display_name: None,
            state_message: None,
            monitoring_tool: "acteon".into(),
            host_name: None,
            state_start_time: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["message_type"], "CRITICAL");
        assert_eq!(json["monitoring_tool"], "acteon");
        assert!(json.get("entity_id").is_none());
        assert!(json.get("entity_display_name").is_none());
        assert!(json.get("state_message").is_none());
        assert!(json.get("host_name").is_none());
    }

    #[test]
    fn alert_request_serializes_full() {
        let req = VictorOpsAlertRequest {
            message_type: VictorOpsMessageType::Recovery,
            entity_id: Some("web-01-high-cpu".into()),
            entity_display_name: Some("High CPU on web-01".into()),
            state_message: Some("CPU > 90% for 5 minutes.".into()),
            monitoring_tool: "prometheus".into(),
            host_name: Some("web-01".into()),
            state_start_time: Some(1_713_897_600),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["message_type"], "RECOVERY");
        assert_eq!(json["entity_id"], "web-01-high-cpu");
        assert_eq!(json["entity_display_name"], "High CPU on web-01");
        assert_eq!(json["state_message"], "CPU > 90% for 5 minutes.");
        assert_eq!(json["monitoring_tool"], "prometheus");
        assert_eq!(json["host_name"], "web-01");
        assert_eq!(json["state_start_time"], 1_713_897_600);
    }

    #[test]
    fn api_response_deserializes() {
        let json = r#"{"result":"success","entity_id":"web-01-high-cpu"}"#;
        let resp: VictorOpsApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.result, "success");
        assert_eq!(resp.entity_id, "web-01-high-cpu");
    }

    #[test]
    fn api_response_tolerates_missing_fields() {
        let resp: VictorOpsApiResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(resp.result, "");
        assert_eq!(resp.entity_id, "");
    }
}
