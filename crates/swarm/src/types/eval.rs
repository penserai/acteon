use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Result of running the eval harness against the workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    /// Scalar fitness score in `[0.0, 1.0]`.
    pub score: f64,
    /// Whether the score meets the configured pass threshold.
    pub passed: bool,
    /// Named metrics extracted from the eval output (e.g., `test_count`, `pass_count`).
    #[serde(default)]
    pub metrics: HashMap<String, f64>,
    /// Raw eval command output (truncated).
    pub output: String,
    /// How long the eval took in seconds.
    pub duration_seconds: f64,
    /// Process exit code.
    pub exit_code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_result_serde_roundtrip() {
        let result = EvalResult {
            score: 0.85,
            passed: true,
            metrics: [("test_count".into(), 42.0), ("pass_count".into(), 36.0)]
                .into_iter()
                .collect(),
            output: "All tests passed".into(),
            duration_seconds: 12.5,
            exit_code: 0,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: EvalResult = serde_json::from_str(&json).unwrap();
        assert!((parsed.score - 0.85).abs() < f64::EPSILON);
        assert!(parsed.passed);
        assert_eq!(parsed.metrics.len(), 2);
    }
}
