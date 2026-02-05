//! State machine configuration types for event lifecycle management.
//!
//! State machines define the possible states an event can be in and the
//! allowed transitions between states. This enables alert-style workflows
//! with lifecycle tracking.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for a state machine.
///
/// State machines are identified by name and define the valid states
/// and transitions for events that use them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StateMachineConfig {
    /// Unique name for this state machine.
    pub name: String,

    /// The initial state for new events.
    pub initial_state: String,

    /// All valid states in this machine.
    pub states: Vec<String>,

    /// Allowed transitions between states.
    pub transitions: Vec<TransitionConfig>,

    /// Automatic timeouts that trigger transitions.
    #[serde(default)]
    pub timeouts: Vec<TimeoutConfig>,
}

impl StateMachineConfig {
    /// Create a new state machine configuration.
    #[must_use]
    pub fn new(name: impl Into<String>, initial_state: impl Into<String>) -> Self {
        let initial = initial_state.into();
        Self {
            name: name.into(),
            initial_state: initial.clone(),
            states: vec![initial],
            transitions: Vec::new(),
            timeouts: Vec::new(),
        }
    }

    /// Add a state to the machine.
    #[must_use]
    pub fn with_state(mut self, state: impl Into<String>) -> Self {
        let state = state.into();
        if !self.states.contains(&state) {
            self.states.push(state);
        }
        self
    }

    /// Add a transition to the machine.
    #[must_use]
    pub fn with_transition(mut self, transition: TransitionConfig) -> Self {
        self.transitions.push(transition);
        self
    }

    /// Add a timeout to the machine.
    #[must_use]
    pub fn with_timeout(mut self, timeout: TimeoutConfig) -> Self {
        self.timeouts.push(timeout);
        self
    }

    /// Check if a state is valid in this machine.
    #[must_use]
    pub fn is_valid_state(&self, state: &str) -> bool {
        self.states.iter().any(|s| s == state)
    }

    /// Check if a transition is allowed.
    #[must_use]
    pub fn is_transition_allowed(&self, from: &str, to: &str) -> bool {
        self.transitions
            .iter()
            .any(|t| t.from == from && t.to == to)
    }

    /// Get the transition config for a specific from->to pair.
    #[must_use]
    pub fn get_transition(&self, from: &str, to: &str) -> Option<&TransitionConfig> {
        self.transitions
            .iter()
            .find(|t| t.from == from && t.to == to)
    }

    /// Get the timeout configuration for a specific state, if any.
    #[must_use]
    pub fn get_timeout_for_state(&self, state: &str) -> Option<&TimeoutConfig> {
        self.timeouts.iter().find(|t| t.in_state == state)
    }
}

/// Configuration for a state transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TransitionConfig {
    /// Source state.
    pub from: String,

    /// Target state.
    pub to: String,

    /// Optional effects to trigger on transition.
    #[serde(default)]
    pub on_transition: TransitionEffects,
}

impl TransitionConfig {
    /// Create a new transition configuration.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            on_transition: TransitionEffects::default(),
        }
    }

    /// Set transition effects.
    #[must_use]
    pub fn with_effects(mut self, effects: TransitionEffects) -> Self {
        self.on_transition = effects;
        self
    }
}

/// Effects to trigger when a transition occurs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TransitionEffects {
    /// Whether to send a notification on this transition.
    #[serde(default)]
    pub notify: bool,

    /// Optional webhook URL to call.
    pub webhook_url: Option<String>,

    /// Additional metadata to attach.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl TransitionEffects {
    /// Create effects that trigger a notification.
    #[must_use]
    pub fn notify() -> Self {
        Self {
            notify: true,
            ..Default::default()
        }
    }
}

/// Configuration for automatic state timeouts.
///
/// When an event remains in a state for longer than the timeout,
/// it automatically transitions to the target state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TimeoutConfig {
    /// The state this timeout applies to.
    pub in_state: String,

    /// Seconds after which the timeout triggers.
    pub after_seconds: u64,

    /// State to transition to when timeout triggers.
    pub transition_to: String,
}

impl TimeoutConfig {
    /// Create a new timeout configuration.
    #[must_use]
    pub fn new(
        in_state: impl Into<String>,
        after_seconds: u64,
        transition_to: impl Into<String>,
    ) -> Self {
        Self {
            in_state: in_state.into(),
            after_seconds,
            transition_to: transition_to.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_creation() {
        let sm = StateMachineConfig::new("alert", "open")
            .with_state("in_progress")
            .with_state("closed")
            .with_transition(TransitionConfig::new("open", "in_progress"))
            .with_transition(TransitionConfig::new("open", "closed"))
            .with_transition(TransitionConfig::new("in_progress", "closed"));

        assert_eq!(sm.name, "alert");
        assert_eq!(sm.initial_state, "open");
        assert_eq!(sm.states.len(), 3);
        assert!(sm.is_valid_state("open"));
        assert!(sm.is_valid_state("closed"));
        assert!(!sm.is_valid_state("invalid"));
    }

    #[test]
    fn transition_validation() {
        let sm = StateMachineConfig::new("ticket", "open")
            .with_state("closed")
            .with_transition(TransitionConfig::new("open", "closed"));

        assert!(sm.is_transition_allowed("open", "closed"));
        assert!(!sm.is_transition_allowed("closed", "open"));
        assert!(!sm.is_transition_allowed("open", "invalid"));
    }

    #[test]
    fn transition_with_effects() {
        let transition =
            TransitionConfig::new("open", "closed").with_effects(TransitionEffects::notify());

        assert!(transition.on_transition.notify);
    }

    #[test]
    fn timeout_configuration() {
        let sm = StateMachineConfig::new("alert", "firing")
            .with_state("resolved")
            .with_timeout(TimeoutConfig::new("firing", 3600, "resolved"));

        assert_eq!(sm.timeouts.len(), 1);
        assert_eq!(sm.timeouts[0].in_state, "firing");
        assert_eq!(sm.timeouts[0].after_seconds, 3600);
    }

    #[test]
    fn serde_roundtrip() {
        let sm = StateMachineConfig::new("test", "initial")
            .with_state("final")
            .with_transition(TransitionConfig::new("initial", "final"));

        let json = serde_json::to_string(&sm).unwrap();
        let back: StateMachineConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(back.name, sm.name);
        assert_eq!(back.states.len(), sm.states.len());
    }
}
