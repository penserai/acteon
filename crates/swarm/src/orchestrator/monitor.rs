use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks per-agent activity to detect stuck agents or infinite loops.
pub struct SwarmMonitor {
    /// Per-agent action counts in the current window.
    agent_actions: HashMap<String, ActionWindow>,
    /// Maximum actions per agent per minute before flagging.
    max_actions_per_minute: u64,
    /// Maximum identical consecutive tool calls before flagging.
    max_repeated_calls: u32,
}

struct ActionWindow {
    count: u64,
    window_start: Instant,
    last_tool: Option<String>,
    repeat_count: u32,
}

/// Alert raised by the monitor.
#[derive(Debug, Clone)]
pub enum MonitorAlert {
    /// Agent is making too many actions per minute.
    HighActionRate {
        agent_id: String,
        actions_per_minute: u64,
    },
    /// Agent is repeatedly calling the same tool.
    RepeatedToolCall {
        agent_id: String,
        tool_name: String,
        repeat_count: u32,
    },
}

impl SwarmMonitor {
    /// Create a new monitor with default thresholds.
    pub fn new() -> Self {
        Self {
            agent_actions: HashMap::new(),
            max_actions_per_minute: 20,
            max_repeated_calls: 5,
        }
    }

    /// Create a monitor with custom thresholds.
    pub fn with_thresholds(max_actions_per_minute: u64, max_repeated_calls: u32) -> Self {
        Self {
            agent_actions: HashMap::new(),
            max_actions_per_minute,
            max_repeated_calls,
        }
    }

    /// Record an action from an agent and return any alerts.
    pub fn record_action(&mut self, agent_id: &str, tool_name: &str) -> Vec<MonitorAlert> {
        let mut alerts = Vec::new();
        let now = Instant::now();

        let window = self
            .agent_actions
            .entry(agent_id.to_string())
            .or_insert_with(|| ActionWindow {
                count: 0,
                window_start: now,
                last_tool: None,
                repeat_count: 0,
            });

        // Reset window if more than 60 seconds old.
        if now.duration_since(window.window_start) > Duration::from_secs(60) {
            window.count = 0;
            window.window_start = now;
        }

        window.count += 1;

        // Check action rate. Only evaluate when at least 1 second of wall
        // time has passed to avoid spurious alerts from initial bursts.
        let elapsed = now.duration_since(window.window_start).as_secs_f64();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rate = if elapsed >= 1.0 {
            (f64::from(u32::try_from(window.count).unwrap_or(u32::MAX)) / elapsed * 60.0) as u64
        } else {
            0
        };
        if rate > self.max_actions_per_minute {
            alerts.push(MonitorAlert::HighActionRate {
                agent_id: agent_id.into(),
                actions_per_minute: rate,
            });
        }

        // Check repeated tool calls.
        if window.last_tool.as_deref() == Some(tool_name) {
            window.repeat_count += 1;
            if window.repeat_count >= self.max_repeated_calls {
                alerts.push(MonitorAlert::RepeatedToolCall {
                    agent_id: agent_id.into(),
                    tool_name: tool_name.into(),
                    repeat_count: window.repeat_count,
                });
            }
        } else {
            window.last_tool = Some(tool_name.into());
            window.repeat_count = 1;
        }

        alerts
    }

    /// Remove tracking for a completed agent.
    pub fn remove_agent(&mut self, agent_id: &str) {
        self.agent_actions.remove(agent_id);
    }
}

impl Default for SwarmMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_alerts_for_normal_usage() {
        let mut monitor = SwarmMonitor::new();
        let alerts = monitor.record_action("agent-1", "Read");
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_repeated_tool_alert() {
        let mut monitor = SwarmMonitor::with_thresholds(100, 3);
        monitor.record_action("agent-1", "Bash");
        monitor.record_action("agent-1", "Bash");
        let alerts = monitor.record_action("agent-1", "Bash");
        assert!(
            alerts
                .iter()
                .any(|a| matches!(a, MonitorAlert::RepeatedToolCall { .. }))
        );
    }

    #[test]
    fn test_different_tools_reset_repeat() {
        // Use a high rate threshold to avoid HighActionRate alerts in tests
        // where all calls happen in the same instant (rate = count * 60 apm).
        let mut monitor = SwarmMonitor::with_thresholds(1000, 3);
        monitor.record_action("agent-1", "Bash");
        monitor.record_action("agent-1", "Bash");
        monitor.record_action("agent-1", "Read"); // Different tool resets.
        let alerts = monitor.record_action("agent-1", "Read");
        // Only 2 repeats of Read, threshold is 3 — no repeated-tool alert.
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_remove_agent() {
        let mut monitor = SwarmMonitor::new();
        monitor.record_action("agent-1", "Bash");
        monitor.remove_agent("agent-1");
        assert!(!monitor.agent_actions.contains_key("agent-1"));
    }
}
