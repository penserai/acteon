use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use crate::config::EvalHarnessConfig;
use crate::error::SwarmError;
use crate::types::eval::EvalResult;

/// Run the eval harness command and parse the output for score signals.
///
/// The command runs as a shell subprocess in the given working directory.
/// Output is scanned for `SCORE:`, `PASS:`, and `WARNINGS:` lines.
/// Falls back to exit-code-based scoring when no signals are found.
pub async fn run_eval_harness(
    config: &EvalHarnessConfig,
    working_dir: &Path,
) -> Result<EvalResult, SwarmError> {
    if config.command.is_empty() {
        return Err(SwarmError::EvalHarness("eval command is empty".into()));
    }

    let start = std::time::Instant::now();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(config.timeout_seconds),
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&config.command)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;

    let duration_seconds = start.elapsed().as_secs_f64();

    match result {
        Ok(Ok(output)) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined: String = format!("{stdout}\n{stderr}").chars().take(10000).collect();

            let (score, metrics) = parse_eval_output(&combined, exit_code);
            let passed = score >= config.pass_threshold;

            tracing::info!(
                score,
                passed,
                exit_code,
                duration = format!("{duration_seconds:.1}s"),
                "eval harness complete"
            );

            Ok(EvalResult {
                score,
                passed,
                metrics,
                output: combined,
                duration_seconds,
                exit_code,
            })
        }
        Ok(Err(e)) => Err(SwarmError::EvalHarness(format!(
            "failed to execute eval command: {e}"
        ))),
        Err(_) => Err(SwarmError::EvalHarness(format!(
            "eval command timed out after {}s",
            config.timeout_seconds
        ))),
    }
}

/// Parse eval output for score signals.
///
/// Supported signals (case-insensitive, last one wins):
/// - `SCORE: <f64>` — direct score assignment
/// - `PASS: <n>/<total>` — computes n/total as score
/// - `WARNINGS: <n>` — each warning reduces score by 0.01
///
/// Fallback: exit code 0 → 1.0, non-zero → 0.0.
fn parse_eval_output(output: &str, exit_code: i32) -> (f64, HashMap<String, f64>) {
    let mut metrics = HashMap::new();
    let mut score: Option<f64> = None;
    let mut warning_count: Option<f64> = None;

    for line in output.lines() {
        let line = line.trim();
        let upper = line.to_uppercase();

        if let Some(rest) = upper.strip_prefix("SCORE:") {
            if let Ok(s) = rest.trim().parse::<f64>() {
                score = Some(s.clamp(0.0, 1.0));
            }
        } else if let Some(rest) = upper.strip_prefix("PASS:") {
            let rest = rest.trim();
            if let Some((n_str, total_str)) = rest.split_once('/')
                && let (Ok(n), Ok(total)) =
                    (n_str.trim().parse::<f64>(), total_str.trim().parse::<f64>())
                && total > 0.0
            {
                metrics.insert("pass_count".into(), n);
                metrics.insert("test_count".into(), total);
                score = Some((n / total).clamp(0.0, 1.0));
            }
        } else if let Some(rest) = upper.strip_prefix("WARNINGS:")
            && let Ok(w) = rest.trim().parse::<f64>()
        {
            warning_count = Some(w);
            metrics.insert("warnings".into(), w);
        }
    }

    let mut final_score = score.unwrap_or(if exit_code == 0 { 1.0 } else { 0.0 });

    // Apply warning penalty (0.01 per warning, clamped to 0.0).
    if let Some(w) = warning_count {
        final_score = (final_score - w * 0.01).max(0.0);
    }

    metrics.insert("exit_code".into(), f64::from(exit_code));

    (final_score, metrics)
}

// ── Git snapshot/revert ────────────────────────────────────────────────────────

const NO_SNAPSHOT: &str = "__no_snapshot__";

/// Create a git stash snapshot of the working directory.
///
/// Returns a stash identifier, or `NO_SNAPSHOT` if there were no changes
/// or the directory is not a git repo.
pub async fn git_snapshot(working_dir: &Path) -> String {
    let label = format!(
        "acteon-swarm-eval-{}",
        chrono::Utc::now().timestamp_millis()
    );

    let result = tokio::process::Command::new("git")
        .args(["stash", "push", "-m", &label, "--include-untracked"])
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("No local changes") || stdout.contains("No stash entries") {
                tracing::debug!("git snapshot: no local changes to stash");
                NO_SNAPSHOT.into()
            } else {
                tracing::info!(label = %label, "git snapshot created");
                label
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("git stash failed: {stderr}");
            NO_SNAPSHOT.into()
        }
        Err(e) => {
            tracing::warn!("git not available: {e}");
            NO_SNAPSHOT.into()
        }
    }
}

/// Revert to a previously created git snapshot (score regressed).
pub async fn git_revert_to_snapshot(working_dir: &Path, stash_ref: &str) {
    if stash_ref == NO_SNAPSHOT {
        return;
    }

    // Discard current changes and restore stash.
    let _ = tokio::process::Command::new("git")
        .args(["checkout", "."])
        .current_dir(working_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;

    let _ = tokio::process::Command::new("git")
        .args(["clean", "-fd"])
        .current_dir(working_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;

    let result = tokio::process::Command::new("git")
        .args(["stash", "pop"])
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!(stash = %stash_ref, "reverted to pre-recovery snapshot");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("git stash pop failed: {stderr}");
        }
        Err(e) => tracing::warn!("git revert failed: {e}"),
    }
}

/// Discard a snapshot after a successful recovery (score improved).
pub async fn git_discard_snapshot(working_dir: &Path, stash_ref: &str) {
    if stash_ref == NO_SNAPSHOT {
        return;
    }

    let result = tokio::process::Command::new("git")
        .args(["stash", "drop"])
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            tracing::debug!(stash = %stash_ref, "discarded snapshot (keeping recovery changes)");
        }
        _ => {
            tracing::debug!("git stash drop failed (may have already been consumed)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_score_signal() {
        let (score, _) = parse_eval_output("some output\nSCORE: 0.85\nmore output", 0);
        assert!((score - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_pass_signal() {
        let (score, metrics) = parse_eval_output("running tests...\nPASS: 42/50\ndone", 0);
        assert!((score - 0.84).abs() < f64::EPSILON);
        assert!((metrics["pass_count"] - 42.0).abs() < f64::EPSILON);
        assert!((metrics["test_count"] - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_warnings_penalty() {
        let (score, metrics) = parse_eval_output("SCORE: 0.90\nWARNINGS: 5", 0);
        assert!((score - 0.85).abs() < f64::EPSILON); // 0.90 - 5*0.01
        assert!((metrics["warnings"] - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_warnings_clamp_to_zero() {
        let (score, _) = parse_eval_output("SCORE: 0.10\nWARNINGS: 50", 0);
        assert!((score).abs() < f64::EPSILON); // clamped to 0.0
    }

    #[test]
    fn test_fallback_exit_code_zero() {
        let (score, _) = parse_eval_output("no score signals here", 0);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_fallback_exit_code_nonzero() {
        let (score, _) = parse_eval_output("error: build failed", 1);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn test_last_score_wins() {
        let (score, _) = parse_eval_output("SCORE: 0.5\nSCORE: 0.9", 0);
        assert!((score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_case_insensitive() {
        let (score, _) = parse_eval_output("score: 0.75", 0);
        assert!((score - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_clamped_to_range() {
        let (score, _) = parse_eval_output("SCORE: 1.5", 0);
        assert!((score - 1.0).abs() < f64::EPSILON);

        let (score, _) = parse_eval_output("SCORE: -0.5", 0);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn test_pass_zero_total_ignored() {
        let (score, _) = parse_eval_output("PASS: 0/0", 0);
        // Zero total is ignored, falls back to exit code.
        assert!((score - 1.0).abs() < f64::EPSILON);
    }
}
