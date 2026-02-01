use thiserror::Error;

/// Errors that can occur during rule parsing, optimization, or evaluation.
#[derive(Debug, Error)]
pub enum RuleError {
    /// A type mismatch occurred during expression evaluation.
    #[error("type error: {0}")]
    TypeError(String),

    /// A referenced variable was not found in the evaluation context.
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),

    /// A called function is not registered as a builtin.
    #[error("undefined function: {0}")]
    UndefinedFunction(String),

    /// An error occurred while accessing the state store.
    #[error("state access error: {0}")]
    StateAccess(String),

    /// A general evaluation error that does not fit other categories.
    #[error("evaluation error: {0}")]
    Evaluation(String),

    /// A parse error when loading rules from a frontend.
    #[error("parse error: {0}")]
    Parse(String),

    /// An invalid regular expression was supplied to a match operation.
    #[error("invalid regex: {0}")]
    InvalidRegex(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        let err = RuleError::TypeError("expected bool, got int".into());
        assert_eq!(err.to_string(), "type error: expected bool, got int");

        let err = RuleError::UndefinedVariable("foo".into());
        assert_eq!(err.to_string(), "undefined variable: foo");

        let err = RuleError::UndefinedFunction("bar".into());
        assert_eq!(err.to_string(), "undefined function: bar");

        let err = RuleError::StateAccess("connection refused".into());
        assert_eq!(err.to_string(), "state access error: connection refused");

        let err = RuleError::Evaluation("division by zero".into());
        assert_eq!(err.to_string(), "evaluation error: division by zero");

        let err = RuleError::Parse("unexpected token".into());
        assert_eq!(err.to_string(), "parse error: unexpected token");

        let err = RuleError::InvalidRegex("unclosed group".into());
        assert_eq!(err.to_string(), "invalid regex: unclosed group");
    }
}
