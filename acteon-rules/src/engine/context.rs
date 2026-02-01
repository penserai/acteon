use std::collections::HashMap;

use chrono::{DateTime, Utc};

use acteon_core::Action;
use acteon_state::StateStore;

/// The evaluation context supplied to the rule engine when evaluating expressions.
///
/// It provides access to the action being evaluated, the state store for
/// stateful lookups (counters, dedup, etc.), environment variables, and the
/// current timestamp.
pub struct EvalContext<'a> {
    /// The action being evaluated.
    pub action: &'a Action,
    /// The state store for stateful rule conditions.
    pub state: &'a dyn StateStore,
    /// Environment variables and external configuration.
    pub environment: &'a HashMap<String, String>,
    /// The current timestamp for time-based evaluations.
    pub now: DateTime<Utc>,
}

impl<'a> EvalContext<'a> {
    /// Create a new evaluation context.
    pub fn new(
        action: &'a Action,
        state: &'a dyn StateStore,
        environment: &'a HashMap<String, String>,
    ) -> Self {
        Self {
            action,
            state,
            environment,
            now: Utc::now(),
        }
    }

    /// Create a new evaluation context with a specific timestamp.
    #[must_use]
    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self
    }
}
