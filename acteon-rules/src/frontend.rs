use crate::error::RuleError;
use crate::ir::rule::Rule;

/// Trait for rule frontends that parse rules from various formats.
///
/// Implementations provide parsing from specific file formats (YAML, JSON, etc.)
/// into the intermediate rule representation.
pub trait RuleFrontend: Send + Sync {
    /// Return the file extensions this frontend supports (e.g., `["yaml", "yml"]`).
    fn extensions(&self) -> &[&str];

    /// Parse rules from a string content.
    fn parse(&self, content: &str) -> Result<Vec<Rule>, RuleError>;

    /// Parse rules from a file path.
    ///
    /// The default implementation reads the file and delegates to [`parse`](Self::parse).
    fn parse_file(&self, path: &std::path::Path) -> Result<Vec<Rule>, RuleError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| RuleError::Parse(format!("cannot read {}: {e}", path.display())))?;
        self.parse(&content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trivial frontend for testing that always returns an empty rule set.
    struct EmptyFrontend;

    impl RuleFrontend for EmptyFrontend {
        fn extensions(&self) -> &[&str] {
            &["test"]
        }

        fn parse(&self, _content: &str) -> Result<Vec<Rule>, RuleError> {
            Ok(vec![])
        }
    }

    #[test]
    fn empty_frontend_extensions() {
        let fe = EmptyFrontend;
        assert_eq!(fe.extensions(), &["test"]);
    }

    #[test]
    fn empty_frontend_parse() {
        let fe = EmptyFrontend;
        let rules = fe.parse("anything").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn empty_frontend_parse_nonexistent_file() {
        let fe = EmptyFrontend;
        let result = fe.parse_file(std::path::Path::new("/nonexistent/path.test"));
        assert!(result.is_err());
    }
}
