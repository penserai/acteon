use serde::{Deserialize, Serialize};

use super::crypto::SecretString;

/// Top-level schema for `auth.toml`.
#[derive(Debug, Deserialize)]
pub struct AuthFileConfig {
    pub settings: AuthSettings,
    #[serde(default)]
    pub users: Vec<UserConfig>,
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,
}

/// Global auth settings.
#[derive(Debug, Deserialize)]
pub struct AuthSettings {
    /// JWT signing secret (may be `ENC[...]` before decryption).
    ///
    /// Wrapped in [`SecretString`] so it is redacted in logs.
    pub jwt_secret: SecretString,
    /// JWT token lifetime in seconds.
    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry_seconds: u64,
}

fn default_jwt_expiry() -> u64 {
    3600
}

/// A user principal that authenticates via username/password and receives a JWT.
#[derive(Debug, Deserialize)]
pub struct UserConfig {
    pub username: String,
    /// Argon2 password hash (may be `ENC[...]` before decryption).
    ///
    /// Wrapped in [`SecretString`] so it is redacted in logs.
    pub password_hash: SecretString,
    /// Role: `"admin"`, `"operator"`, or `"viewer"`.
    pub role: String,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

/// A resource-level grant scoped to tenants, namespaces, providers, and action types.
///
/// A caller is authorized for an action when **every** dimension on at least
/// one of their grants matches the action. Each field supports the `"*"`
/// wildcard.
///
/// Tenant matching is hierarchical: a grant on tenant `"acme"` also covers
/// `"acme.us-east"`, `"acme.us-east.prod"`, and so on. This lets operators
/// scope API keys to a parent tenant without enumerating every sub-tenant.
/// Hierarchical matching is one-way: a grant on `"acme.us-east"` does *not*
/// cover `"acme"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// Tenant identifiers (or tenant prefixes), or `["*"]` for all.
    pub tenants: Vec<String>,
    /// Namespace identifiers, or `["*"]` for all.
    pub namespaces: Vec<String>,
    /// Provider identifiers, or `["*"]` for all.
    ///
    /// Defaults to `["*"]` when omitted from `auth.toml` for backward
    /// compatibility with grants written before provider scoping existed.
    #[serde(default = "wildcard_vec")]
    pub providers: Vec<String>,
    /// Action type identifiers, or `["*"]` for all.
    pub actions: Vec<String>,
}

fn wildcard_vec() -> Vec<String> {
    vec!["*".to_owned()]
}

impl Grant {
    /// Check whether this grant matches a specific
    /// `(tenant, namespace, provider, action_type)` tuple.
    ///
    /// - Tenant matching is hierarchical (prefix + `.` separator) in addition
    ///   to exact and wildcard match.
    /// - Namespace, provider, and `action_type` support exact and wildcard
    ///   matching only.
    pub fn matches(
        &self,
        tenant: &str,
        namespace: &str,
        provider: &str,
        action_type: &str,
    ) -> bool {
        tenant_matches(&self.tenants, tenant)
            && dimension_matches(&self.namespaces, namespace)
            && dimension_matches(&self.providers, provider)
            && dimension_matches(&self.actions, action_type)
    }
}

/// Match a dimension against a grant pattern list.
///
/// Returns `true` if the list contains `"*"` or an exact match for `value`.
fn dimension_matches(patterns: &[String], value: &str) -> bool {
    patterns.iter().any(|p| p == "*" || p == value)
}

/// Match a tenant against a grant's tenant pattern list.
///
/// Supports wildcard (`"*"`), exact match, and hierarchical prefix match:
/// a pattern `"acme"` matches tenants `"acme"`, `"acme.us-east"`, and
/// `"acme.us-east.prod"`. Prefix matching requires a `.` separator after the
/// pattern to avoid accidentally matching `"acme-corp"` against `"acme"`.
pub fn tenant_matches(patterns: &[String], tenant: &str) -> bool {
    patterns.iter().any(|p| {
        if p == "*" || p == tenant {
            return true;
        }
        // Hierarchical: pattern is a strict prefix of tenant, followed by `.`.
        tenant.len() > p.len() + 1
            && tenant.starts_with(p.as_str())
            && tenant.as_bytes()[p.len()] == b'.'
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(tenants: &[&str], namespaces: &[&str], providers: &[&str], actions: &[&str]) -> Grant {
        Grant {
            tenants: tenants.iter().map(|s| (*s).to_string()).collect(),
            namespaces: namespaces.iter().map(|s| (*s).to_string()).collect(),
            providers: providers.iter().map(|s| (*s).to_string()).collect(),
            actions: actions.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn wildcard_grant_matches_anything() {
        let g = grant(&["*"], &["*"], &["*"], &["*"]);
        assert!(g.matches("acme", "prod", "email", "send"));
        assert!(g.matches("anything", "foo", "bar", "baz"));
    }

    #[test]
    fn exact_match_on_all_dimensions() {
        let g = grant(&["acme"], &["prod"], &["email"], &["send"]);
        assert!(g.matches("acme", "prod", "email", "send"));
        assert!(!g.matches("acme", "prod", "email", "draft"));
        assert!(!g.matches("acme", "prod", "sms", "send"));
        assert!(!g.matches("acme", "staging", "email", "send"));
        assert!(!g.matches("other", "prod", "email", "send"));
    }

    #[test]
    fn hierarchical_tenant_matching() {
        let g = grant(&["acme"], &["*"], &["*"], &["*"]);
        assert!(g.matches("acme", "x", "y", "z"), "exact match");
        assert!(g.matches("acme.us-east", "x", "y", "z"), "one-level child");
        assert!(
            g.matches("acme.us-east.prod", "x", "y", "z"),
            "multi-level child"
        );
    }

    #[test]
    fn hierarchical_matching_is_not_prefix_of_unrelated_tenant() {
        let g = grant(&["acme"], &["*"], &["*"], &["*"]);
        // Without a `.` separator, "acme-corp" should NOT match grant "acme".
        assert!(!g.matches("acme-corp", "x", "y", "z"));
        assert!(!g.matches("acmecorp", "x", "y", "z"));
    }

    #[test]
    fn hierarchical_matching_is_one_way() {
        // A grant for a child should NOT cover the parent.
        let g = grant(&["acme.us-east"], &["*"], &["*"], &["*"]);
        assert!(g.matches("acme.us-east", "x", "y", "z"));
        assert!(g.matches("acme.us-east.prod", "x", "y", "z"));
        assert!(!g.matches("acme", "x", "y", "z"));
        assert!(!g.matches("acme.eu-west", "x", "y", "z"));
    }

    #[test]
    fn provider_scoping_enforced() {
        let g = grant(&["*"], &["*"], &["email"], &["*"]);
        assert!(g.matches("any", "any", "email", "any"));
        assert!(!g.matches("any", "any", "sms", "any"));
    }

    #[test]
    fn providers_defaults_to_wildcard_when_deserialized_without_field() {
        // Backward compat: existing auth.toml without `providers` should
        // still grant access to any provider.
        let toml = r#"
tenants = ["acme"]
namespaces = ["prod"]
actions = ["*"]
"#;
        let g: Grant = toml::from_str(toml).expect("parse");
        assert_eq!(g.providers, vec!["*".to_string()]);
        assert!(g.matches("acme", "prod", "email", "send"));
        assert!(g.matches("acme", "prod", "sms", "send"));
    }

    #[test]
    fn tenant_matches_helper_direct() {
        let patterns = vec!["acme".to_string()];
        assert!(tenant_matches(&patterns, "acme"));
        assert!(tenant_matches(&patterns, "acme.us-east"));
        assert!(!tenant_matches(&patterns, "acme-corp"));
        assert!(!tenant_matches(&patterns, "acm"));
    }
}

/// An API key principal that authenticates via `X-API-Key` header.
#[derive(Debug, Deserialize)]
pub struct ApiKeyConfig {
    pub name: String,
    /// SHA-256 hash of the raw key (may be `ENC[...]` before decryption).
    ///
    /// Wrapped in [`SecretString`] so it is redacted in logs.
    pub key_hash: SecretString,
    /// Role: `"admin"`, `"operator"`, or `"viewer"`.
    pub role: String,
    #[serde(default)]
    pub grants: Vec<Grant>,
}
