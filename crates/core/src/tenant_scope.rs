//! Hierarchical tenant authorization scope, shared across audit/analytics
//! backends.
//!
//! A *scope* is the set of tenant grant patterns a caller is authorized to read
//! (e.g. `["acme", "globex"]`). It is set by the server from the caller's grants
//! — never from client input — and threaded into queries so each backend
//! restricts results to the caller's authorized tenants.
//!
//! Matching is hierarchical and segment-aware, mirroring grant semantics: a
//! pattern `acme` covers `acme` and any descendant (`acme.us-east`,
//! `acme.prod.eu`, …) but NOT the sibling `acmecorp`.
//!
//! An empty scope means "unrestricted" (a wildcard-tenant caller). A scope must
//! never contain `"*"` — represent wildcard access with an empty scope instead.

/// Returns `true` if `tenant` is within the authorization `scope`.
///
/// An empty `scope` is unrestricted and matches every tenant. Otherwise the
/// tenant must equal, or be a hierarchical descendant of, at least one pattern.
#[must_use]
pub fn tenant_in_scope(scope: &[String], tenant: &str) -> bool {
    scope.is_empty() || scope.iter().any(|p| tenant_covered_by(p, tenant))
}

/// Returns `true` if grant pattern `p` covers `tenant` hierarchically: an exact
/// match, or `tenant` is a dot-delimited descendant of `p`.
///
/// Segment-aware: `acme` covers `acme` and `acme.prod` but not `acmecorp`. This
/// mirrors the server's grant `tenant_matches` semantics (kept in sync).
#[must_use]
pub fn tenant_covered_by(p: &str, tenant: &str) -> bool {
    if p == tenant {
        return true;
    }
    // Descendant: `tenant` is `p` followed by a `.` separator and more.
    tenant.len() > p.len() + 1 && tenant.as_bytes()[p.len()] == b'.' && tenant.starts_with(p)
}

/// Escape a literal string for safe use inside a SQL `LIKE` pattern, escaping
/// the metacharacters `\`, `%`, and `_` with `\` (the default `LIKE` escape
/// character for both `PostgreSQL` and `ClickHouse`).
#[must_use]
pub fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Build the `LIKE` pattern matching every hierarchical descendant of
/// `pattern` (i.e. `pattern.<anything>`), with the literal portion escaped.
/// Pair with an exact `tenant = pattern` check for full subtree coverage.
#[must_use]
pub fn like_descendants_pattern(pattern: &str) -> String {
    format!("{}.%", escape_like(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn empty_scope_is_unrestricted() {
        assert!(tenant_in_scope(&[], "anything"));
        assert!(tenant_in_scope(&[], "acme.prod"));
    }

    #[test]
    fn exact_and_descendant_match() {
        let scope = s(&["acme"]);
        assert!(tenant_in_scope(&scope, "acme"));
        assert!(tenant_in_scope(&scope, "acme.prod"));
        assert!(tenant_in_scope(&scope, "acme.prod.eu"));
    }

    #[test]
    fn sibling_does_not_match() {
        let scope = s(&["acme"]);
        assert!(!tenant_in_scope(&scope, "acmecorp"));
        assert!(!tenant_in_scope(&scope, "acme-corp"));
        assert!(!tenant_in_scope(&scope, "globex"));
    }

    #[test]
    fn union_across_patterns() {
        let scope = s(&["acme", "globex"]);
        assert!(tenant_in_scope(&scope, "acme.us-east"));
        assert!(tenant_in_scope(&scope, "globex"));
        assert!(tenant_in_scope(&scope, "globex.eu"));
        assert!(!tenant_in_scope(&scope, "initech"));
    }

    #[test]
    fn like_escaping() {
        assert_eq!(escape_like("ac_me"), "ac\\_me");
        assert_eq!(escape_like("ac%me"), "ac\\%me");
        assert_eq!(escape_like("a\\b"), "a\\\\b");
        assert_eq!(like_descendants_pattern("acme"), "acme.%");
        assert_eq!(like_descendants_pattern("ac_me"), "ac\\_me.%");
    }
}
