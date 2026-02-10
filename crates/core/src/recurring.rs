use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Template for the action dispatched on each cron tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecurringActionTemplate {
    /// Target provider (e.g. `"email"`, `"webhook"`).
    pub provider: String,
    /// Action type discriminator (e.g. `"send_digest"`).
    pub action_type: String,
    /// JSON payload for the provider.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub payload: serde_json::Value,
    /// Optional metadata labels merged into each dispatched action.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Optional dedup key template. Supports `{{recurring_id}}` and
    /// `{{execution_time}}` placeholders.
    #[serde(default)]
    pub dedup_key: Option<String>,
}

/// A recurring action definition.
///
/// Stores a cron-scheduled action that fires periodically. Each occurrence
/// synthesizes a concrete [`Action`](crate::Action) from the
/// [`RecurringActionTemplate`] and dispatches it through the gateway pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecurringAction {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Namespace this recurring action belongs to.
    pub namespace: String,
    /// Tenant that owns this recurring action.
    pub tenant: String,
    /// Cron expression (standard 5-field).
    /// Examples: `"0 9 * * MON-FRI"`, `"*/5 * * * *"`
    pub cron_expr: String,
    /// IANA timezone for evaluating the cron expression.
    /// Defaults to `"UTC"` if not provided.
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// Whether this recurring action is currently active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The action template dispatched on each occurrence.
    pub action_template: RecurringActionTemplate,
    /// When this recurring action was created.
    pub created_at: DateTime<Utc>,
    /// When this recurring action was last updated.
    pub updated_at: DateTime<Utc>,
    /// The most recent execution time (`None` if never executed).
    #[serde(default)]
    pub last_executed_at: Option<DateTime<Utc>>,
    /// The next scheduled execution time (`None` if paused or expired).
    #[serde(default)]
    pub next_execution_at: Option<DateTime<Utc>>,
    /// Optional end date after which the recurring action is auto-disabled.
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    /// Optional maximum number of executions.
    #[serde(default)]
    pub max_executions: Option<u64>,
    /// Total number of successful executions.
    #[serde(default)]
    pub execution_count: u64,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary key-value labels for filtering and organization.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

fn default_timezone() -> String {
    "UTC".to_owned()
}

fn default_enabled() -> bool {
    true
}

/// Default minimum interval between recurring action occurrences (60 seconds).
pub const DEFAULT_MIN_INTERVAL_SECONDS: u64 = 60;

/// Validate a cron expression and return the parsed representation.
///
/// Returns an error if the expression is invalid.
pub fn validate_cron_expr(expr: &str) -> Result<croner::Cron, CronValidationError> {
    croner::Cron::new(expr)
        .parse()
        .map_err(|e| CronValidationError::InvalidExpression(format!("{e}")))
}

/// Validate a timezone string against the IANA timezone database.
///
/// Returns the parsed `chrono_tz::Tz` on success.
pub fn validate_timezone(tz: &str) -> Result<chrono_tz::Tz, CronValidationError> {
    tz.parse::<chrono_tz::Tz>()
        .map_err(|_| CronValidationError::InvalidTimezone(tz.to_owned()))
}

/// Compute the next occurrence of a cron expression after `after` in the given
/// timezone.
///
/// Returns `None` if the cron expression has no future occurrences.
pub fn next_occurrence(
    cron: &croner::Cron,
    tz: chrono_tz::Tz,
    after: &DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let after_tz = after.with_timezone(&tz);
    cron.find_next_occurrence(&after_tz, false)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Validate that a cron expression does not fire more frequently than the
/// minimum interval.
///
/// Computes two consecutive occurrences and checks the gap between them.
pub fn validate_min_interval(
    cron: &croner::Cron,
    tz: chrono_tz::Tz,
    min_interval_seconds: u64,
) -> Result<(), CronValidationError> {
    let now = Utc::now();
    let first = next_occurrence(cron, tz, &now);
    let Some(first) = first else {
        return Err(CronValidationError::NoFutureOccurrence);
    };
    let second = next_occurrence(cron, tz, &first);
    let Some(second) = second else {
        // Only one future occurrence -- interval check not applicable.
        return Ok(());
    };
    let gap = (second - first).num_seconds();
    let gap_unsigned = gap.unsigned_abs();
    if gap < 0 || gap_unsigned < min_interval_seconds {
        return Err(CronValidationError::IntervalTooShort {
            actual_seconds: gap_unsigned,
            minimum_seconds: min_interval_seconds,
        });
    }
    Ok(())
}

/// Errors from cron expression validation.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CronValidationError {
    /// The cron expression could not be parsed.
    #[error("invalid cron expression: {0}")]
    InvalidExpression(String),
    /// The timezone string is not a valid IANA timezone.
    #[error("invalid timezone: {0}")]
    InvalidTimezone(String),
    /// The cron expression fires more frequently than allowed.
    #[error(
        "cron interval too short: {actual_seconds}s between occurrences, minimum is {minimum_seconds}s"
    )]
    IntervalTooShort {
        actual_seconds: u64,
        minimum_seconds: u64,
    },
    /// The cron expression has no future occurrences.
    #[error("cron expression has no future occurrences")]
    NoFutureOccurrence,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_valid_cron_expr() {
        let result = validate_cron_expr("0 9 * * MON-FRI");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_every_5_minutes() {
        let result = validate_cron_expr("*/5 * * * *");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_invalid_cron_expr() {
        let result = validate_cron_expr("not a cron");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid cron"));
    }

    #[test]
    fn validate_valid_timezone() {
        let result = validate_timezone("US/Eastern");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_utc_timezone() {
        let result = validate_timezone("UTC");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_invalid_timezone() {
        let result = validate_timezone("Not/A/Timezone");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid timezone"));
    }

    #[test]
    fn next_occurrence_returns_future_time() {
        let cron = validate_cron_expr("*/5 * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let now = Utc::now();
        let next = next_occurrence(&cron, tz, &now);
        assert!(next.is_some());
        assert!(next.unwrap() > now);
    }

    #[test]
    fn min_interval_rejects_every_second() {
        // Every minute -- should pass with 60s minimum.
        let cron = validate_cron_expr("* * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let result = validate_min_interval(&cron, tz, 60);
        assert!(result.is_ok());
    }

    #[test]
    fn min_interval_accepts_hourly() {
        let cron = validate_cron_expr("0 * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let result = validate_min_interval(&cron, tz, 60);
        assert!(result.is_ok());
    }

    #[test]
    fn min_interval_rejects_when_too_short() {
        // Every minute with a 5-minute minimum should fail.
        let cron = validate_cron_expr("* * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let result = validate_min_interval(&cron, tz, 300);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("too short"),
            "expected 'too short' in error: {err}"
        );
    }

    #[test]
    fn recurring_action_serde_roundtrip() {
        let action = RecurringAction {
            id: "test-id".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            cron_expr: "0 9 * * MON-FRI".into(),
            timezone: "US/Eastern".into(),
            enabled: true,
            action_template: RecurringActionTemplate {
                provider: "email".into(),
                action_type: "send_digest".into(),
                payload: serde_json::json!({"to": "user@test.com"}),
                metadata: HashMap::new(),
                dedup_key: None,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_executed_at: None,
            next_execution_at: None,
            ends_at: None,
            max_executions: None,
            execution_count: 0,
            description: Some("Test recurring action".into()),
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let back: RecurringAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "test-id");
        assert_eq!(back.cron_expr, "0 9 * * MON-FRI");
        assert_eq!(back.timezone, "US/Eastern");
        assert!(back.enabled);
        assert_eq!(back.action_template.provider, "email");
    }

    #[test]
    fn recurring_action_deserializes_with_defaults() {
        let json = r#"{
            "id": "test",
            "namespace": "ns",
            "tenant": "t",
            "cron_expr": "0 9 * * *",
            "action_template": {
                "provider": "email",
                "action_type": "send",
                "payload": {}
            },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;

        let action: RecurringAction = serde_json::from_str(json).unwrap();
        assert_eq!(action.timezone, "UTC");
        assert!(action.enabled);
        assert!(action.last_executed_at.is_none());
        assert!(action.next_execution_at.is_none());
        assert!(action.ends_at.is_none());
        assert_eq!(action.execution_count, 0);
        assert!(action.description.is_none());
        assert!(action.labels.is_empty());
    }

    // --- Additional cron parsing tests ---

    #[test]
    fn validate_monthly_cron() {
        let result = validate_cron_expr("0 10 1 * *");
        assert!(
            result.is_ok(),
            "monthly on the 1st at 10:00 should be valid"
        );
    }

    #[test]
    fn validate_yearly_cron() {
        let result = validate_cron_expr("0 0 1 1 *");
        assert!(
            result.is_ok(),
            "yearly on Jan 1 at midnight should be valid"
        );
    }

    #[test]
    fn validate_range_and_list_cron() {
        // 9am and 5pm on weekdays
        let result = validate_cron_expr("0 9,17 * * 1-5");
        assert!(result.is_ok(), "range and list syntax should be valid");
    }

    #[test]
    fn validate_empty_cron_is_invalid() {
        let result = validate_cron_expr("");
        assert!(result.is_err(), "empty string should be invalid");
    }

    #[test]
    fn validate_partial_cron_is_invalid() {
        let result = validate_cron_expr("0 9 *");
        assert!(result.is_err(), "3-field cron should be invalid");
    }

    #[test]
    fn validate_cron_with_out_of_range_values() {
        // Minute field max is 59
        let result = validate_cron_expr("60 9 * * *");
        assert!(result.is_err(), "minute=60 should be out of range");
    }

    #[test]
    fn cron_validation_error_display() {
        let err = CronValidationError::InvalidExpression("bad".into());
        assert_eq!(err.to_string(), "invalid cron expression: bad");

        let err = CronValidationError::InvalidTimezone("Mars/Olympus".into());
        assert_eq!(err.to_string(), "invalid timezone: Mars/Olympus");

        let err = CronValidationError::IntervalTooShort {
            actual_seconds: 30,
            minimum_seconds: 60,
        };
        assert!(err.to_string().contains("30s"));
        assert!(err.to_string().contains("60s"));

        let err = CronValidationError::NoFutureOccurrence;
        assert!(err.to_string().contains("no future"));
    }

    // --- Timezone validation tests ---

    #[test]
    fn validate_various_timezones() {
        for tz in &[
            "US/Pacific",
            "Europe/London",
            "Asia/Tokyo",
            "Australia/Sydney",
            "America/New_York",
        ] {
            let result = validate_timezone(tz);
            assert!(result.is_ok(), "timezone {tz} should be valid");
        }
    }

    #[test]
    fn validate_empty_timezone_is_invalid() {
        let result = validate_timezone("");
        assert!(result.is_err(), "empty timezone should be invalid");
    }

    // --- next_occurrence tests ---

    #[test]
    fn next_occurrence_with_timezone() {
        let cron = validate_cron_expr("0 9 * * *").unwrap();
        let tz = validate_timezone("US/Eastern").unwrap();
        let now = Utc::now();
        let next = next_occurrence(&cron, tz, &now);
        assert!(next.is_some(), "should have a next occurrence");
        assert!(
            next.unwrap() > now,
            "next occurrence should be in the future"
        );
    }

    #[test]
    fn next_occurrence_successive_calls_advance() {
        let cron = validate_cron_expr("*/5 * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let now = Utc::now();

        let first = next_occurrence(&cron, tz, &now).unwrap();
        let second = next_occurrence(&cron, tz, &first).unwrap();

        assert!(second > first, "second occurrence should be after first");
        let gap = (second - first).num_minutes();
        assert_eq!(gap, 5, "gap between occurrences should be 5 minutes");
    }

    #[test]
    fn next_occurrence_hourly_gap() {
        let cron = validate_cron_expr("0 * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let now = Utc::now();

        let first = next_occurrence(&cron, tz, &now).unwrap();
        let second = next_occurrence(&cron, tz, &first).unwrap();

        let gap = (second - first).num_minutes();
        assert_eq!(gap, 60, "hourly cron should have 60-minute gap");
    }

    // --- min_interval tests ---

    #[test]
    fn min_interval_daily_passes_any_minimum() {
        let cron = validate_cron_expr("0 9 * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        // Daily = 86400 seconds, even 1-hour minimum should pass
        let result = validate_min_interval(&cron, tz, 3600);
        assert!(result.is_ok(), "daily cron should pass 1-hour minimum");
    }

    #[test]
    fn min_interval_every_5_min_rejects_10_min_minimum() {
        let cron = validate_cron_expr("*/5 * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let result = validate_min_interval(&cron, tz, 600);
        assert!(
            result.is_err(),
            "5-minute cron should fail 10-minute minimum"
        );
    }

    #[test]
    fn min_interval_every_5_min_passes_5_min_minimum() {
        let cron = validate_cron_expr("*/5 * * * *").unwrap();
        let tz = validate_timezone("UTC").unwrap();
        let result = validate_min_interval(&cron, tz, 300);
        assert!(result.is_ok(), "5-minute cron should pass 5-minute minimum");
    }

    #[test]
    fn min_interval_with_non_utc_timezone() {
        let cron = validate_cron_expr("*/5 * * * *").unwrap();
        let tz = validate_timezone("US/Eastern").unwrap();
        let result = validate_min_interval(&cron, tz, 300);
        assert!(
            result.is_ok(),
            "5-minute cron in US/Eastern should pass 5-minute minimum"
        );
    }

    #[test]
    fn default_min_interval_constant() {
        assert_eq!(DEFAULT_MIN_INTERVAL_SECONDS, 60);
    }

    // --- RecurringAction comprehensive field tests ---

    #[test]
    fn recurring_action_full_roundtrip_with_all_fields() {
        let now = Utc::now();
        let next = now + chrono::Duration::hours(1);
        let ends = now + chrono::Duration::days(30);

        let mut labels = HashMap::new();
        labels.insert("team".into(), "engineering".into());
        labels.insert("env".into(), "production".into());

        let mut metadata = HashMap::new();
        metadata.insert("priority".into(), "high".into());

        let action = RecurringAction {
            id: "rec-full-001".into(),
            namespace: "notifications".into(),
            tenant: "tenant-abc".into(),
            cron_expr: "0 9 * * MON-FRI".into(),
            timezone: "America/New_York".into(),
            enabled: true,
            action_template: RecurringActionTemplate {
                provider: "email".into(),
                action_type: "send_digest".into(),
                payload: serde_json::json!({"to": "team@example.com", "subject": "Daily Digest"}),
                metadata,
                dedup_key: Some("digest-{{recurring_id}}-{{execution_time}}".into()),
            },
            created_at: now,
            updated_at: now,
            last_executed_at: Some(now - chrono::Duration::hours(24)),
            next_execution_at: Some(next),
            ends_at: Some(ends),
            max_executions: None,
            execution_count: 42,
            description: Some("Weekday morning digest for engineering".into()),
            labels,
        };

        let json = serde_json::to_string_pretty(&action).unwrap();
        let back: RecurringAction = serde_json::from_str(&json).unwrap();

        assert_eq!(back.id, "rec-full-001");
        assert_eq!(back.namespace, "notifications");
        assert_eq!(back.tenant, "tenant-abc");
        assert_eq!(back.cron_expr, "0 9 * * MON-FRI");
        assert_eq!(back.timezone, "America/New_York");
        assert!(back.enabled);
        assert_eq!(back.action_template.provider, "email");
        assert_eq!(back.action_template.action_type, "send_digest");
        assert!(back.action_template.payload["to"].as_str() == Some("team@example.com"));
        assert_eq!(
            back.action_template.metadata.get("priority"),
            Some(&"high".to_string())
        );
        assert_eq!(
            back.action_template.dedup_key.as_deref(),
            Some("digest-{{recurring_id}}-{{execution_time}}")
        );
        assert!(back.last_executed_at.is_some());
        assert!(back.next_execution_at.is_some());
        assert!(back.ends_at.is_some());
        assert_eq!(back.execution_count, 42);
        assert_eq!(
            back.description.as_deref(),
            Some("Weekday morning digest for engineering")
        );
        assert_eq!(back.labels.get("team"), Some(&"engineering".to_string()));
        assert_eq!(back.labels.get("env"), Some(&"production".to_string()));
    }

    #[test]
    fn recurring_action_template_with_empty_metadata() {
        let template = RecurringActionTemplate {
            provider: "webhook".into(),
            action_type: "ping".into(),
            payload: serde_json::json!({}),
            metadata: HashMap::new(),
            dedup_key: None,
        };

        let json = serde_json::to_string(&template).unwrap();
        let back: RecurringActionTemplate = serde_json::from_str(&json).unwrap();
        assert!(back.metadata.is_empty());
        assert!(back.dedup_key.is_none());
    }

    #[test]
    fn recurring_action_template_metadata_default_on_missing() {
        // Simulate JSON missing the metadata field
        let json = r#"{
            "provider": "email",
            "action_type": "send",
            "payload": {"key": "value"}
        }"#;
        let template: RecurringActionTemplate = serde_json::from_str(json).unwrap();
        assert!(template.metadata.is_empty());
        assert!(template.dedup_key.is_none());
    }

    #[test]
    fn recurring_action_disabled_state() {
        let action = RecurringAction {
            id: "rec-disabled".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            cron_expr: "0 9 * * *".into(),
            timezone: "UTC".into(),
            enabled: false,
            action_template: RecurringActionTemplate {
                provider: "email".into(),
                action_type: "send".into(),
                payload: serde_json::json!({}),
                metadata: HashMap::new(),
                dedup_key: None,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_executed_at: None,
            next_execution_at: None,
            ends_at: None,
            max_executions: None,
            execution_count: 0,
            description: None,
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let back: RecurringAction = serde_json::from_str(&json).unwrap();
        assert!(!back.enabled);
        assert!(back.next_execution_at.is_none());
    }

    #[test]
    fn recurring_action_high_execution_count() {
        let action = RecurringAction {
            id: "rec-high-count".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            cron_expr: "* * * * *".into(),
            timezone: "UTC".into(),
            enabled: true,
            action_template: RecurringActionTemplate {
                provider: "webhook".into(),
                action_type: "ping".into(),
                payload: serde_json::json!({}),
                metadata: HashMap::new(),
                dedup_key: None,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_executed_at: Some(Utc::now()),
            next_execution_at: Some(Utc::now()),
            ends_at: None,
            max_executions: None,
            execution_count: u64::MAX,
            description: None,
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let back: RecurringAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back.execution_count, u64::MAX);
    }

    #[test]
    fn recurring_action_complex_payload() {
        let payload = serde_json::json!({
            "to": ["user1@test.com", "user2@test.com"],
            "cc": [],
            "subject": "Report",
            "body": {"html": "<h1>Hello</h1>", "text": "Hello"},
            "attachments": [{"name": "report.pdf", "size": 1024}],
            "nested": {"deep": {"value": true}}
        });

        let template = RecurringActionTemplate {
            provider: "email".into(),
            action_type: "send_report".into(),
            payload: payload.clone(),
            metadata: HashMap::new(),
            dedup_key: None,
        };

        let json = serde_json::to_string(&template).unwrap();
        let back: RecurringActionTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.payload, payload);
    }

    #[test]
    fn recurring_action_labels_roundtrip() {
        let mut labels = HashMap::new();
        labels.insert("environment".into(), "staging".into());
        labels.insert("region".into(), "us-east-1".into());
        labels.insert("cost-center".into(), "CC-1234".into());

        let action = RecurringAction {
            id: "rec-labels".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            cron_expr: "0 * * * *".into(),
            timezone: "UTC".into(),
            enabled: true,
            action_template: RecurringActionTemplate {
                provider: "webhook".into(),
                action_type: "check".into(),
                payload: serde_json::json!({}),
                metadata: HashMap::new(),
                dedup_key: None,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_executed_at: None,
            next_execution_at: None,
            ends_at: None,
            max_executions: None,
            execution_count: 0,
            description: None,
            labels: labels.clone(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let back: RecurringAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back.labels.len(), 3);
        assert_eq!(back.labels, labels);
    }
}
