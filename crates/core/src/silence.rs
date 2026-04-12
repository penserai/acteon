//! Silences — time-bounded label-pattern mutes for dispatched actions.
//!
//! A [`Silence`] is a tenant-scoped rule that suppresses any dispatched
//! action whose `metadata.labels` match all of the silence's [`SilenceMatcher`]s
//! within the silence's active time window. Silences are typically used
//! during maintenance windows to temporarily mute alerts without modifying
//! the rule set.
//!
//! Silences are evaluated after rule evaluation but before provider dispatch,
//! so the audit trail still records the rule verdict that *would* have
//! applied. This lets operators trace silenced actions with full forensic
//! context.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// Maximum number of characters allowed in a regex matcher pattern.
///
/// Bounds worst-case DFA compilation cost and keeps matcher patterns
/// human-reviewable.
pub const MAX_REGEX_PATTERN_LEN: usize = 256;

/// Maximum compiled regex DFA size in bytes.
pub const MAX_REGEX_SIZE: usize = 65_536;

/// Match operator for a single silence matcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MatchOp {
    /// Label value exactly equals the matcher value.
    Equal,
    /// Label value is not exactly equal to the matcher value, or label is absent.
    NotEqual,
    /// Label value matches the matcher value as a regex (anchored).
    Regex,
    /// Label value does not match the matcher value as a regex (anchored), or label is absent.
    NotRegex,
}

impl std::fmt::Display for MatchOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::Regex => "=~",
            Self::NotRegex => "!~",
        })
    }
}

/// A single matcher in a silence. Multiple matchers in one silence are
/// combined with AND semantics — the silence matches an action only if
/// *all* matchers match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SilenceMatcher {
    /// Label name to match against `action.metadata.labels`.
    pub name: String,
    /// Literal value (for `Equal` / `NotEqual`) or regex pattern (for `Regex` / `NotRegex`).
    pub value: String,
    /// Match operator.
    pub op: MatchOp,
}

impl SilenceMatcher {
    /// Build a new matcher, validating regex patterns for length and
    /// compilation cost.
    ///
    /// # Errors
    ///
    /// Returns an error string if the matcher is invalid. For regex operators,
    /// this includes patterns longer than [`MAX_REGEX_PATTERN_LEN`] characters
    /// and patterns whose compiled DFA exceeds [`MAX_REGEX_SIZE`] bytes.
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        op: MatchOp,
    ) -> Result<Self, String> {
        let matcher = Self {
            name: name.into(),
            value: value.into(),
            op,
        };
        matcher.validate()?;
        Ok(matcher)
    }

    /// Validate the matcher without constructing it.
    ///
    /// # Errors
    ///
    /// Returns an error string describing the first validation failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("matcher name must not be empty".to_owned());
        }

        match self.op {
            MatchOp::Regex | MatchOp::NotRegex => {
                if self.value.len() > MAX_REGEX_PATTERN_LEN {
                    return Err(format!(
                        "regex pattern exceeds {MAX_REGEX_PATTERN_LEN}-character limit"
                    ));
                }
                // Compile the regex once as a validation step. Reject
                // patterns that exceed the size or DFA complexity cap.
                compile_regex(&self.value)?;
            }
            MatchOp::Equal | MatchOp::NotEqual => {}
        }

        Ok(())
    }

    /// Check whether this matcher matches the given label map.
    #[must_use]
    pub fn matches_labels(&self, labels: &HashMap<String, String>) -> bool {
        let label_value = labels.get(&self.name).map(String::as_str);

        match self.op {
            MatchOp::Equal => label_value == Some(self.value.as_str()),
            MatchOp::NotEqual => label_value != Some(self.value.as_str()),
            MatchOp::Regex => {
                // Missing labels do not satisfy a positive regex match.
                let Some(value) = label_value else {
                    return false;
                };
                compile_regex(&self.value)
                    .map(|re| re.is_match(value))
                    .unwrap_or(false)
            }
            MatchOp::NotRegex => {
                // Missing labels satisfy a negative regex match (they
                // trivially do not match the pattern).
                let Some(value) = label_value else {
                    return true;
                };
                compile_regex(&self.value)
                    .map(|re| !re.is_match(value))
                    .unwrap_or(true)
            }
        }
    }
}

impl std::fmt::Display for SilenceMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}\"{}\"", self.name, self.op, self.value)
    }
}

/// Compile a regex with the same size / complexity caps used for silence
/// validation. Anchored at both ends (`^...$`) so matchers always match
/// the whole label value rather than any substring.
fn compile_regex(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(MAX_REGEX_SIZE)
        .dfa_size_limit(MAX_REGEX_SIZE)
        .build()
        .map_err(|e| format!("invalid regex pattern: {e}"))
}

/// A silence — a tenant-scoped time-bounded mute that suppresses dispatched
/// actions whose labels match all of the silence's matchers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Silence {
    /// Unique identifier (UUID v7, assigned on creation).
    pub id: String,
    /// Namespace this silence applies to.
    pub namespace: String,
    /// Tenant this silence applies to.
    pub tenant: String,
    /// Matchers — all must match for the silence to apply.
    pub matchers: Vec<SilenceMatcher>,
    /// When the silence becomes active.
    pub starts_at: DateTime<Utc>,
    /// When the silence expires.
    pub ends_at: DateTime<Utc>,
    /// Identity of the caller that created the silence.
    pub created_by: String,
    /// Human-readable comment explaining why the silence exists.
    pub comment: String,
    /// When this silence was created.
    pub created_at: DateTime<Utc>,
    /// When this silence was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Silence {
    /// Check whether this silence is currently active at the given time.
    #[must_use]
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        now >= self.starts_at && now < self.ends_at
    }

    /// Check whether this silence matches an action with the given labels.
    ///
    /// The silence must have at least one matcher, and *every* matcher
    /// must match the provided labels. An empty matcher list never matches
    /// (guards against accidentally muting everything).
    #[must_use]
    pub fn matches_labels(&self, labels: &HashMap<String, String>) -> bool {
        if self.matchers.is_empty() {
            return false;
        }
        self.matchers.iter().all(|m| m.matches_labels(labels))
    }

    /// Full match check: silence is active at `now` AND its matchers match
    /// the given labels.
    #[must_use]
    pub fn applies_to(&self, labels: &HashMap<String, String>, now: DateTime<Utc>) -> bool {
        self.is_active_at(now) && self.matches_labels(labels)
    }

    /// Validate the silence as a whole.
    ///
    /// # Errors
    ///
    /// Returns an error string if the silence has no matchers, invalid
    /// matchers, or `ends_at <= starts_at`.
    pub fn validate(&self) -> Result<(), String> {
        if self.matchers.is_empty() {
            return Err("silence must have at least one matcher".to_owned());
        }
        if self.ends_at <= self.starts_at {
            return Err("silence ends_at must be after starts_at".to_owned());
        }
        for matcher in &self.matchers {
            matcher.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    // =========================================================================
    // SilenceMatcher — individual operator semantics
    // =========================================================================

    #[test]
    fn equal_matcher_matches_exact_value() {
        let m = SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap();
        assert!(m.matches_labels(&labels(&[("severity", "warning")])));
        assert!(!m.matches_labels(&labels(&[("severity", "critical")])));
        assert!(!m.matches_labels(&labels(&[])));
    }

    #[test]
    fn not_equal_matcher_matches_absent_or_different_value() {
        let m = SilenceMatcher::new("severity", "warning", MatchOp::NotEqual).unwrap();
        assert!(!m.matches_labels(&labels(&[("severity", "warning")])));
        assert!(m.matches_labels(&labels(&[("severity", "critical")])));
        assert!(m.matches_labels(&labels(&[])));
    }

    #[test]
    fn regex_matcher_is_anchored() {
        let m = SilenceMatcher::new("severity", "warn.*", MatchOp::Regex).unwrap();
        assert!(m.matches_labels(&labels(&[("severity", "warning")])));
        assert!(m.matches_labels(&labels(&[("severity", "warn")])));
        // Anchored: substring does not match if there is content after.
        assert!(!m.matches_labels(&labels(&[("severity", "prewarning")])));
    }

    #[test]
    fn regex_matcher_handles_alternation() {
        let m = SilenceMatcher::new("severity", "warning|critical", MatchOp::Regex).unwrap();
        assert!(m.matches_labels(&labels(&[("severity", "warning")])));
        assert!(m.matches_labels(&labels(&[("severity", "critical")])));
        assert!(!m.matches_labels(&labels(&[("severity", "info")])));
    }

    #[test]
    fn regex_matcher_rejects_missing_label() {
        let m = SilenceMatcher::new("severity", ".*", MatchOp::Regex).unwrap();
        // Missing label does not satisfy a positive regex match.
        assert!(!m.matches_labels(&labels(&[])));
    }

    #[test]
    fn not_regex_matcher_accepts_missing_label() {
        let m = SilenceMatcher::new("severity", "critical", MatchOp::NotRegex).unwrap();
        // Missing label trivially does not match the pattern.
        assert!(m.matches_labels(&labels(&[])));
        assert!(m.matches_labels(&labels(&[("severity", "warning")])));
        assert!(!m.matches_labels(&labels(&[("severity", "critical")])));
    }

    // =========================================================================
    // Matcher validation
    // =========================================================================

    #[test]
    fn empty_name_is_rejected() {
        let result = SilenceMatcher::new("", "warning", MatchOp::Equal);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn oversized_regex_pattern_is_rejected() {
        let long_pattern = "a".repeat(MAX_REGEX_PATTERN_LEN + 1);
        let result = SilenceMatcher::new("severity", long_pattern, MatchOp::Regex);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds"));
    }

    #[test]
    fn pattern_at_limit_is_accepted() {
        // Use a simple character-class pattern at the max length.
        let long_pattern = "a".repeat(MAX_REGEX_PATTERN_LEN);
        let result = SilenceMatcher::new("severity", long_pattern, MatchOp::Regex);
        assert!(result.is_ok());
    }

    #[test]
    fn malformed_regex_is_rejected() {
        let result = SilenceMatcher::new("severity", "[unclosed", MatchOp::Regex);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid regex"));
    }

    #[test]
    fn literal_value_matcher_does_not_validate_as_regex() {
        // `[unclosed` is fine as an Equal matcher — it's a literal string.
        let result = SilenceMatcher::new("label", "[unclosed", MatchOp::Equal);
        assert!(result.is_ok());
    }

    // =========================================================================
    // Silence — compound match semantics
    // =========================================================================

    fn silence_with_matchers(matchers: Vec<SilenceMatcher>) -> Silence {
        let now = Utc::now();
        Silence {
            id: "test".to_owned(),
            namespace: "prod".to_owned(),
            tenant: "acme".to_owned(),
            matchers,
            starts_at: now - chrono::Duration::hours(1),
            ends_at: now + chrono::Duration::hours(1),
            created_by: "test".to_owned(),
            comment: "test".to_owned(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn multiple_matchers_are_anded() {
        let s = silence_with_matchers(vec![
            SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap(),
            SilenceMatcher::new("team", "platform", MatchOp::Equal).unwrap(),
        ]);

        // Both match → silenced.
        assert!(s.matches_labels(&labels(&[("severity", "warning"), ("team", "platform"),])));
        // Only one matches → not silenced.
        assert!(!s.matches_labels(&labels(&[("severity", "warning"), ("team", "database"),])));
        // Neither matches → not silenced.
        assert!(!s.matches_labels(&labels(&[("severity", "critical"), ("team", "database"),])));
    }

    #[test]
    fn empty_matcher_list_never_matches() {
        // Guard against accidentally muting everything with a bare silence.
        let s = silence_with_matchers(vec![]);
        assert!(!s.matches_labels(&labels(&[("severity", "warning")])));
    }

    #[test]
    fn silence_is_active_only_inside_window() {
        let now = Utc::now();
        let mut s = silence_with_matchers(vec![
            SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap(),
        ]);

        // Now (inside window from setup): active.
        assert!(s.is_active_at(now));

        // Ends in the past → not active.
        s.ends_at = now - chrono::Duration::minutes(30);
        assert!(!s.is_active_at(now));

        // Starts in the future → not active.
        s.starts_at = now + chrono::Duration::minutes(30);
        s.ends_at = now + chrono::Duration::hours(2);
        assert!(!s.is_active_at(now));
    }

    #[test]
    fn applies_to_checks_both_time_and_labels() {
        let now = Utc::now();
        let mut s = silence_with_matchers(vec![
            SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap(),
        ]);

        // Active + matching labels → applies.
        assert!(s.applies_to(&labels(&[("severity", "warning")]), now));

        // Active + non-matching labels → does not apply.
        assert!(!s.applies_to(&labels(&[("severity", "critical")]), now));

        // Expired + matching labels → does not apply.
        s.ends_at = now - chrono::Duration::minutes(1);
        assert!(!s.applies_to(&labels(&[("severity", "warning")]), now));
    }

    #[test]
    fn validate_rejects_empty_matchers() {
        let s = silence_with_matchers(vec![]);
        let err = s.validate().unwrap_err();
        assert!(err.contains("at least one matcher"));
    }

    #[test]
    fn validate_rejects_inverted_time_range() {
        let now = Utc::now();
        let s = Silence {
            id: "test".to_owned(),
            namespace: "prod".to_owned(),
            tenant: "acme".to_owned(),
            matchers: vec![SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap()],
            starts_at: now,
            ends_at: now - chrono::Duration::hours(1),
            created_by: "test".to_owned(),
            comment: "test".to_owned(),
            created_at: now,
            updated_at: now,
        };
        let err = s.validate().unwrap_err();
        assert!(err.contains("ends_at"));
    }

    #[test]
    fn matcher_display_format() {
        let m = SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap();
        assert_eq!(m.to_string(), "severity=\"warning\"");

        let m = SilenceMatcher::new("severity", "warn.*", MatchOp::Regex).unwrap();
        assert_eq!(m.to_string(), "severity=~\"warn.*\"");
    }
}
