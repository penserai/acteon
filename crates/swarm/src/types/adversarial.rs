use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::AgentEngine;

/// Severity level of an adversarial challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChallengeSeverity {
    /// Informational observation, unlikely to cause issues.
    Low,
    /// Moderate concern that should be addressed.
    Medium,
    /// Serious issue that must be resolved.
    High,
    /// Critical flaw that blocks completion.
    Critical,
}

impl ChallengeSeverity {
    /// Convert to a numeric score in `[0.0, 1.0]`.
    pub fn score(self) -> f64 {
        match self {
            Self::Low => 0.25,
            Self::Medium => 0.5,
            Self::High => 0.75,
            Self::Critical => 1.0,
        }
    }

    /// Parse from a numeric score, rounding to the nearest severity level.
    pub fn from_score(score: f64) -> Self {
        if score >= 0.875 {
            Self::Critical
        } else if score >= 0.625 {
            Self::High
        } else if score >= 0.375 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

/// A single challenge raised by the adversarial swarm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialChallenge {
    /// Unique challenge identifier.
    pub id: String,
    /// Which task this challenge targets (if task-specific).
    pub target_task_id: Option<String>,
    /// Category of the challenge (e.g., "correctness", "security", "performance").
    pub category: String,
    /// Human-readable description of the issue.
    pub description: String,
    /// Severity level.
    pub severity: ChallengeSeverity,
    /// Numeric severity score in `[0.0, 1.0]`.
    pub severity_score: f64,
    /// Suggested remediation (from the adversarial agent).
    pub suggested_fix: Option<String>,
    /// Whether this challenge was resolved in the recovery phase.
    #[serde(default)]
    pub resolved: bool,
    /// Resolution summary (from the recovery agent).
    #[serde(default)]
    pub resolution: Option<String>,
}

/// One complete challenge-recovery cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialRound {
    /// Round number (1-indexed).
    pub round: usize,
    /// Engine used for the challenge phase.
    pub challenge_engine: AgentEngine,
    /// Engine used for the recovery phase.
    pub recovery_engine: AgentEngine,
    /// Challenges raised in this round.
    pub challenges: Vec<AdversarialChallenge>,
    /// Number of challenges that exceeded the severity threshold.
    pub actionable_challenges: usize,
    /// Number of challenges resolved in recovery.
    pub resolved_count: usize,
    /// When the challenge phase started.
    pub challenge_started_at: DateTime<Utc>,
    /// When the recovery phase completed.
    pub recovery_finished_at: Option<DateTime<Utc>>,
}

impl AdversarialRound {
    /// Returns true if all actionable challenges were resolved.
    pub fn fully_resolved(&self) -> bool {
        self.resolved_count >= self.actionable_challenges
    }
}

/// Aggregated result of the adversarial process across all rounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialResult {
    /// All rounds executed.
    pub rounds: Vec<AdversarialRound>,
    /// Total challenges raised across all rounds.
    pub total_challenges: usize,
    /// Total challenges resolved across all rounds.
    pub total_resolved: usize,
    /// Whether the adversarial process considers the output acceptable.
    pub accepted: bool,
    /// Remaining unresolved challenges (if any).
    pub unresolved: Vec<AdversarialChallenge>,
}

impl AdversarialResult {
    /// Build from completed rounds, filtering unresolved challenges above threshold.
    pub fn from_rounds(rounds: Vec<AdversarialRound>, severity_threshold: f64) -> Self {
        let total_challenges: usize = rounds.iter().map(|r| r.challenges.len()).sum();
        let total_resolved: usize = rounds.iter().map(|r| r.resolved_count).sum();

        let unresolved: Vec<AdversarialChallenge> = rounds
            .iter()
            .flat_map(|r| r.challenges.iter())
            .filter(|c| !c.resolved && c.severity_score >= severity_threshold)
            .cloned()
            .collect();

        let accepted = unresolved.is_empty();

        Self {
            rounds,
            total_challenges,
            total_resolved,
            accepted,
            unresolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_score_roundtrip() {
        assert_eq!(ChallengeSeverity::from_score(0.25), ChallengeSeverity::Low);
        assert_eq!(
            ChallengeSeverity::from_score(0.5),
            ChallengeSeverity::Medium
        );
        assert_eq!(ChallengeSeverity::from_score(0.75), ChallengeSeverity::High);
        assert_eq!(
            ChallengeSeverity::from_score(1.0),
            ChallengeSeverity::Critical
        );
    }

    #[test]
    fn test_severity_from_score_boundaries() {
        assert_eq!(ChallengeSeverity::from_score(0.0), ChallengeSeverity::Low);
        assert_eq!(ChallengeSeverity::from_score(0.374), ChallengeSeverity::Low);
        assert_eq!(
            ChallengeSeverity::from_score(0.375),
            ChallengeSeverity::Medium
        );
        assert_eq!(
            ChallengeSeverity::from_score(0.624),
            ChallengeSeverity::Medium
        );
        assert_eq!(
            ChallengeSeverity::from_score(0.625),
            ChallengeSeverity::High
        );
        assert_eq!(
            ChallengeSeverity::from_score(0.874),
            ChallengeSeverity::High
        );
        assert_eq!(
            ChallengeSeverity::from_score(0.875),
            ChallengeSeverity::Critical
        );
    }

    #[test]
    fn test_adversarial_round_fully_resolved() {
        let round = AdversarialRound {
            round: 1,
            challenge_engine: AgentEngine::Gemini,
            recovery_engine: AgentEngine::Claude,
            challenges: vec![],
            actionable_challenges: 3,
            resolved_count: 3,
            challenge_started_at: Utc::now(),
            recovery_finished_at: Some(Utc::now()),
        };
        assert!(round.fully_resolved());

        let partial = AdversarialRound {
            resolved_count: 2,
            ..round
        };
        assert!(!partial.fully_resolved());
    }

    #[test]
    fn test_adversarial_result_from_rounds() {
        let challenge_a = AdversarialChallenge {
            id: "c1".into(),
            target_task_id: Some("t1".into()),
            category: "correctness".into(),
            description: "Missing error handling".into(),
            severity: ChallengeSeverity::High,
            severity_score: 0.75,
            suggested_fix: Some("Add error handling".into()),
            resolved: true,
            resolution: Some("Added try-catch".into()),
        };
        let challenge_b = AdversarialChallenge {
            id: "c2".into(),
            target_task_id: None,
            category: "performance".into(),
            description: "N+1 query".into(),
            severity: ChallengeSeverity::Medium,
            severity_score: 0.5,
            suggested_fix: None,
            resolved: false,
            resolution: None,
        };

        let round = AdversarialRound {
            round: 1,
            challenge_engine: AgentEngine::Gemini,
            recovery_engine: AgentEngine::Claude,
            challenges: vec![challenge_a, challenge_b],
            actionable_challenges: 2,
            resolved_count: 1,
            challenge_started_at: Utc::now(),
            recovery_finished_at: Some(Utc::now()),
        };

        // Threshold 0.5: challenge_b (0.5 >= 0.5) is unresolved.
        let result = AdversarialResult::from_rounds(vec![round.clone()], 0.5);
        assert!(!result.accepted);
        assert_eq!(result.unresolved.len(), 1);
        assert_eq!(result.unresolved[0].id, "c2");

        // Threshold 0.6: challenge_b (0.5 < 0.6) is below threshold.
        let result = AdversarialResult::from_rounds(vec![round], 0.6);
        assert!(result.accepted);
        assert!(result.unresolved.is_empty());
    }

    #[test]
    fn test_challenge_serde_roundtrip() {
        let challenge = AdversarialChallenge {
            id: "c1".into(),
            target_task_id: Some("t1".into()),
            category: "correctness".into(),
            description: "Test issue".into(),
            severity: ChallengeSeverity::High,
            severity_score: 0.75,
            suggested_fix: Some("Fix it".into()),
            resolved: false,
            resolution: None,
        };
        let json = serde_json::to_string(&challenge).unwrap();
        let parsed: AdversarialChallenge = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "c1");
        assert_eq!(parsed.severity, ChallengeSeverity::High);
    }
}
