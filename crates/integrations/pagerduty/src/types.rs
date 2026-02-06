use serde::{Deserialize, Serialize};

/// A link to display in the `PagerDuty` incident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagerDutyLink {
    /// The URL of the link.
    pub href: String,
    /// Optional text to display for the link.
    pub text: Option<String>,
}

/// An image to display in the `PagerDuty` incident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagerDutyImage {
    /// The URL of the image.
    pub src: String,
    /// Optional URL to make the image a link.
    pub href: Option<String>,
    /// Optional alternative text for the image.
    pub alt: Option<String>,
}

/// Request body for the `PagerDuty` Events API v2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagerDutyEvent {
    /// Integration routing key.
    pub routing_key: String,

    /// Event action: `"trigger"`, `"acknowledge"`, or `"resolve"`.
    pub event_action: String,

    /// Deduplication key for correlating trigger/ack/resolve events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,

    /// Event payload (required for trigger events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<PagerDutyPayload>,

    /// Images to display in the `PagerDuty` incident.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<PagerDutyImage>>,

    /// Links to display in the `PagerDuty` incident.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<PagerDutyLink>>,
}

/// Payload section of a `PagerDuty` trigger event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagerDutyPayload {
    /// Brief description of the event.
    pub summary: String,

    /// The source of the event (e.g. hostname or service name).
    pub source: String,

    /// Severity level: `"critical"`, `"error"`, `"warning"`, or `"info"`.
    pub severity: String,

    /// Logical grouping component (e.g. `"web-01"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,

    /// Logical grouping (e.g. `"production"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Event class/type (e.g. `"cpu"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,

    /// Arbitrary key-value details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_details: Option<serde_json::Value>,
}

/// Response from the `PagerDuty` Events API v2.
#[derive(Debug, Clone, Deserialize)]
pub struct PagerDutyApiResponse {
    /// Status string (`"success"` on success).
    pub status: String,

    /// Human-readable message.
    pub message: String,

    /// Deduplication key assigned by `PagerDuty` (present on success).
    pub dedup_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_trigger_with_payload() {
        let event = PagerDutyEvent {
            routing_key: "R123".into(),
            event_action: "trigger".into(),
            dedup_key: Some("dedup-1".into()),
            payload: Some(PagerDutyPayload {
                summary: "High CPU".into(),
                source: "web-01".into(),
                severity: "critical".into(),
                component: Some("cpu".into()),
                group: None,
                class: None,
                custom_details: None,
            }),
            images: None,
            links: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["routing_key"], "R123");
        assert_eq!(json["event_action"], "trigger");
        assert_eq!(json["dedup_key"], "dedup-1");
        assert_eq!(json["payload"]["summary"], "High CPU");
        assert_eq!(json["payload"]["severity"], "critical");
        assert!(json["payload"].get("group").is_none());
    }

    #[test]
    fn event_serializes_acknowledge_without_payload() {
        let event = PagerDutyEvent {
            routing_key: "R123".into(),
            event_action: "acknowledge".into(),
            dedup_key: Some("dedup-1".into()),
            payload: None,
            images: None,
            links: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_action"], "acknowledge");
        assert_eq!(json["dedup_key"], "dedup-1");
        assert!(json.get("payload").is_none());
    }

    #[test]
    fn api_response_deserializes_success() {
        let json = r#"{"status":"success","message":"Event processed","dedup_key":"abc123"}"#;
        let resp: PagerDutyApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "success");
        assert_eq!(resp.message, "Event processed");
        assert_eq!(resp.dedup_key.as_deref(), Some("abc123"));
    }

    #[test]
    fn api_response_deserializes_error() {
        let json =
            r#"{"status":"invalid event","message":"Event object is invalid","dedup_key":null}"#;
        let resp: PagerDutyApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "invalid event");
        assert!(resp.dedup_key.is_none());
    }

    #[test]
    fn payload_omits_none_fields() {
        let payload = PagerDutyPayload {
            summary: "test".into(),
            source: "src".into(),
            severity: "info".into(),
            component: None,
            group: None,
            class: None,
            custom_details: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json.get("component").is_none());
        assert!(json.get("group").is_none());
        assert!(json.get("class").is_none());
        assert!(json.get("custom_details").is_none());
    }

    #[test]
    fn event_serializes_with_images_and_links() {
        let event = PagerDutyEvent {
            routing_key: "R123".into(),
            event_action: "trigger".into(),
            dedup_key: None,
            payload: Some(PagerDutyPayload {
                summary: "test".into(),
                source: "src".into(),
                severity: "info".into(),
                component: None,
                group: None,
                class: None,
                custom_details: None,
            }),
            images: Some(vec![PagerDutyImage {
                src: "https://example.com/image.png".into(),
                href: Some("https://example.com/".into()),
                alt: Some("alt text".into()),
            }]),
            links: Some(vec![PagerDutyLink {
                href: "https://example.com/".into(),
                text: Some("link text".into()),
            }]),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["images"][0]["src"], "https://example.com/image.png");
        assert_eq!(json["links"][0]["href"], "https://example.com/");
    }
}
