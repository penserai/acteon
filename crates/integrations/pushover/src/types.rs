use serde::{Deserialize, Serialize};

/// Pushover priority level.
///
/// `Emergency` notifications bypass the recipient's quiet hours
/// **and** require `retry` + `expire` parameters — the server keeps
/// re-notifying the user every `retry` seconds until they
/// acknowledge the alert (by tapping it), or `expire` seconds
/// elapse, whichever comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PushoverPriority {
    /// `-2`: no notification at all, the message appears silently
    /// in the history.
    Lowest,
    /// `-1`: deliver silently (no sound, no vibration).
    Low,
    /// `0`: normal priority — the default.
    #[default]
    Normal,
    /// `1`: high priority — bypasses the user's quiet hours.
    High,
    /// `2`: emergency priority — bypasses quiet hours **and**
    /// requires acknowledgment. Must be paired with `retry` and
    /// `expire` fields.
    Emergency,
}

impl PushoverPriority {
    /// Parse an integer priority value.
    ///
    /// # Errors
    ///
    /// Returns an error message when `raw` is outside the
    /// `-2..=2` range.
    pub fn from_i32(raw: i32) -> Result<Self, String> {
        match raw {
            -2 => Ok(Self::Lowest),
            -1 => Ok(Self::Low),
            0 => Ok(Self::Normal),
            1 => Ok(Self::High),
            2 => Ok(Self::Emergency),
            other => Err(format!(
                "invalid Pushover priority {other}: must be in -2..=2"
            )),
        }
    }

    /// Convert back to the wire integer representation.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Lowest => -2,
            Self::Low => -1,
            Self::Normal => 0,
            Self::High => 1,
            Self::Emergency => 2,
        }
    }
}

/// Serialized form of a Pushover message request.
///
/// Pushover's API is form-encoded (`application/x-www-form-urlencoded`),
/// **not** JSON, so this struct is deliberately flat — no nested
/// objects, no arrays. `serde_urlencoded` (which reqwest's `.form()`
/// helper uses under the hood) can't serialize anything that needs
/// indirection beyond a single level.
///
/// `None` fields are skipped so the Pushover API sees only the
/// parameters the caller actually provided.
#[derive(Debug, Clone, Serialize)]
pub struct PushoverRequest {
    /// Application token (`T...`) — the app identifier.
    pub token: String,
    /// User or group key (`U...` / `G...`) — the recipient.
    pub user: String,
    /// Body of the notification.
    pub message: String,

    /// Optional notification title shown above the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Priority level as the wire integer (`-2..=2`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    /// Seconds between re-notifications when `priority == 2`
    /// (minimum 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u32>,
    /// Seconds until the server gives up re-notifying when
    /// `priority == 2` (maximum 10800).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire: Option<u32>,
    /// Notification sound name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    /// Supplementary URL attached to the notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Label for `url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_title: Option<String>,
    /// Specific device name to deliver to (overrides the default
    /// of every device on the recipient's account).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Render the message body as HTML (`1` = yes, omitted = no).
    /// Mutually exclusive with `monospace`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<u8>,
    /// Render the message body as monospace. Mutually exclusive
    /// with `html`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monospace: Option<u8>,
    /// Unix timestamp (seconds) of the originating event — the
    /// Pushover app shows this instead of the receive time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// Auto-delete the message from the client after `ttl`
    /// seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
}

/// Response body from the Pushover Messages API.
///
/// `status` is `1` on success and `0` on failure. Error responses
/// additionally populate the `errors` array with one or more
/// human-readable strings describing what went wrong.
#[derive(Debug, Clone, Deserialize)]
pub struct PushoverApiResponse {
    /// `1` on success, `0` on failure. This is the Pushover-layer
    /// status, independent of the HTTP status.
    #[serde(default)]
    pub status: i32,
    /// Request ID assigned by the Pushover server (UUID).
    #[serde(default)]
    pub request: String,
    /// Optional `receipt` ID, present for emergency-priority
    /// notifications so clients can poll acknowledgment state.
    #[serde(default)]
    pub receipt: String,
    /// Array of error strings when `status == 0`.
    #[serde(default)]
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_parse_valid() {
        assert_eq!(
            PushoverPriority::from_i32(-2).unwrap(),
            PushoverPriority::Lowest
        );
        assert_eq!(
            PushoverPriority::from_i32(-1).unwrap(),
            PushoverPriority::Low
        );
        assert_eq!(
            PushoverPriority::from_i32(0).unwrap(),
            PushoverPriority::Normal
        );
        assert_eq!(
            PushoverPriority::from_i32(1).unwrap(),
            PushoverPriority::High
        );
        assert_eq!(
            PushoverPriority::from_i32(2).unwrap(),
            PushoverPriority::Emergency
        );
    }

    #[test]
    fn priority_parse_out_of_range() {
        assert!(PushoverPriority::from_i32(3).is_err());
        assert!(PushoverPriority::from_i32(-3).is_err());
    }

    #[test]
    fn priority_roundtrip_i32() {
        for raw in -2..=2 {
            let p = PushoverPriority::from_i32(raw).unwrap();
            assert_eq!(p.as_i32(), raw);
        }
    }

    #[test]
    fn priority_default_is_normal() {
        assert_eq!(PushoverPriority::default(), PushoverPriority::Normal);
    }

    #[test]
    fn request_serializes_form_minimum() {
        // The form-encoded output is what goes on the wire.
        let req = PushoverRequest {
            token: "app-token".into(),
            user: "user-key".into(),
            message: "Deploy complete".into(),
            title: None,
            priority: None,
            retry: None,
            expire: None,
            sound: None,
            url: None,
            url_title: None,
            device: None,
            html: None,
            monospace: None,
            timestamp: None,
            ttl: None,
        };
        let form = serde_urlencoded::to_string(&req).unwrap();
        // Field order may vary, so test with `contains`.
        assert!(form.contains("token=app-token"));
        assert!(form.contains("user=user-key"));
        assert!(form.contains("message=Deploy+complete"));
        // Skipped fields must not appear on the wire.
        assert!(!form.contains("title="));
        assert!(!form.contains("priority="));
        assert!(!form.contains("retry="));
    }

    #[test]
    fn request_serializes_form_full() {
        let req = PushoverRequest {
            token: "t".into(),
            user: "u".into(),
            message: "CPU >90% on web-01".into(),
            title: Some("High CPU".into()),
            priority: Some(2),
            retry: Some(60),
            expire: Some(3600),
            sound: Some("cashregister".into()),
            url: Some("https://runbook.example.com/cpu".into()),
            url_title: Some("Runbook".into()),
            device: None,
            html: Some(0),
            monospace: None,
            timestamp: Some(1_713_897_600),
            ttl: Some(7200),
        };
        let form = serde_urlencoded::to_string(&req).unwrap();
        assert!(form.contains("message=CPU+%3E90%25+on+web-01"));
        assert!(form.contains("priority=2"));
        assert!(form.contains("retry=60"));
        assert!(form.contains("expire=3600"));
        assert!(form.contains("sound=cashregister"));
        assert!(form.contains("url_title=Runbook"));
        assert!(form.contains("timestamp=1713897600"));
        assert!(form.contains("ttl=7200"));
        assert!(form.contains("html=0"));
    }

    #[test]
    fn api_response_deserializes_success() {
        let json = r#"{"status":1,"request":"abc-123"}"#;
        let resp: PushoverApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, 1);
        assert_eq!(resp.request, "abc-123");
        assert!(resp.errors.is_empty());
    }

    #[test]
    fn api_response_deserializes_error() {
        let json = r#"{"status":0,"request":"abc","errors":["user identifier is invalid"]}"#;
        let resp: PushoverApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, 0);
        assert_eq!(resp.errors.len(), 1);
        assert_eq!(resp.errors[0], "user identifier is invalid");
    }

    #[test]
    fn api_response_deserializes_with_receipt() {
        // Emergency-priority (priority=2) notifications come back
        // with a receipt so clients can poll acknowledgment state.
        let json = r#"{"status":1,"request":"abc","receipt":"rec-xyz"}"#;
        let resp: PushoverApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.receipt, "rec-xyz");
    }

    #[test]
    fn api_response_tolerates_missing_fields() {
        let resp: PushoverApiResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(resp.status, 0);
        assert!(resp.errors.is_empty());
    }
}
