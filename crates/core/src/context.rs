use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::action::Action;

/// Context provided to the rules engine when evaluating an action.
#[derive(Debug, Clone)]
pub struct ActionContext {
    /// The action being evaluated.
    pub action: Action,
    /// Environment variables / external context.
    pub environment: HashMap<String, String>,
    /// Evaluation timestamp.
    pub timestamp: DateTime<Utc>,
}

impl ActionContext {
    /// Create a new context for the given action.
    #[must_use]
    pub fn new(action: Action) -> Self {
        Self {
            action,
            environment: HashMap::new(),
            timestamp: Utc::now(),
        }
    }

    /// Add an environment variable.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Set the environment map.
    #[must_use]
    pub fn with_environment(mut self, env: HashMap<String, String>) -> Self {
        self.environment = env;
        self
    }
}
