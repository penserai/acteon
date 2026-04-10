use acteon_core::Caller;

use super::config::Grant;
use super::role::Role;

/// Rich server-side caller identity extracted from authentication.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// Caller identifier (username or API key name).
    pub id: String,
    /// The principal's role.
    pub role: Role,
    /// The principal's resource grants.
    pub grants: Vec<Grant>,
    /// Authentication method (`"jwt"`, `"api_key"`, or `"anonymous"`).
    pub auth_method: String,
}

impl CallerIdentity {
    /// Build an anonymous identity with full admin access (used when auth is disabled).
    pub fn anonymous() -> Self {
        Self {
            id: String::new(),
            role: Role::Admin,
            grants: vec![Grant {
                tenants: vec!["*".to_owned()],
                namespaces: vec!["*".to_owned()],
                providers: vec!["*".to_owned()],
                actions: vec!["*".to_owned()],
            }],
            auth_method: "anonymous".to_owned(),
        }
    }

    /// Check if this caller is authorized for a specific
    /// `(tenant, namespace, provider, action_type)` tuple.
    ///
    /// Tenant matching is hierarchical: a grant on `"acme"` also covers
    /// `"acme.us-east"`. See [`Grant::matches`] for details.
    pub fn is_authorized(
        &self,
        tenant: &str,
        namespace: &str,
        provider: &str,
        action_type: &str,
    ) -> bool {
        self.grants
            .iter()
            .any(|g| g.matches(tenant, namespace, provider, action_type))
    }

    /// Return the set of tenant patterns this caller has access to, for
    /// filtering audit queries. Returns `None` if the caller has wildcard
    /// tenant access.
    ///
    /// Note: the returned strings are grant *patterns*, not resolved
    /// tenants. Because tenant grants are hierarchical, a returned value of
    /// `"acme"` means the caller can read any tenant matching `acme` or
    /// `acme.*`. Query-filter consumers should treat them as prefixes.
    pub fn allowed_tenants(&self) -> Option<Vec<&str>> {
        let mut tenants = Vec::new();
        for g in &self.grants {
            if g.tenants.iter().any(|t| t == "*") {
                return None; // wildcard — no filtering needed
            }
            for t in &g.tenants {
                if !tenants.contains(&t.as_str()) {
                    tenants.push(t.as_str());
                }
            }
        }
        Some(tenants)
    }

    /// Return allowed namespaces, or `None` if the caller has wildcard namespace access.
    pub fn allowed_namespaces(&self) -> Option<Vec<&str>> {
        let mut namespaces = Vec::new();
        for g in &self.grants {
            if g.namespaces.iter().any(|n| n == "*") {
                return None;
            }
            for n in &g.namespaces {
                if !namespaces.contains(&n.as_str()) {
                    namespaces.push(n.as_str());
                }
            }
        }
        Some(namespaces)
    }

    /// Return allowed providers, or `None` if the caller has wildcard provider access.
    pub fn allowed_providers(&self) -> Option<Vec<&str>> {
        let mut providers = Vec::new();
        for g in &self.grants {
            if g.providers.iter().any(|p| p == "*") {
                return None;
            }
            for p in &g.providers {
                if !providers.contains(&p.as_str()) {
                    providers.push(p.as_str());
                }
            }
        }
        Some(providers)
    }

    /// Convert to the minimal `Caller` for audit threading.
    pub fn to_caller(&self) -> Caller {
        Caller {
            id: self.id.clone(),
            auth_method: self.auth_method.clone(),
        }
    }
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

    fn identity_with(grants: Vec<Grant>) -> CallerIdentity {
        CallerIdentity {
            id: "test".into(),
            role: Role::Admin,
            grants,
            auth_method: "test".into(),
        }
    }

    #[test]
    fn anonymous_is_authorized_for_everything() {
        let id = CallerIdentity::anonymous();
        assert!(id.is_authorized("any", "any", "any", "any"));
    }

    #[test]
    fn is_authorized_enforces_provider_dimension() {
        let id = identity_with(vec![grant(&["acme"], &["prod"], &["email"], &["send"])]);
        assert!(id.is_authorized("acme", "prod", "email", "send"));
        // Provider mismatch → deny, even if everything else matches.
        assert!(!id.is_authorized("acme", "prod", "sms", "send"));
    }

    #[test]
    fn is_authorized_matches_hierarchical_tenants() {
        let id = identity_with(vec![grant(&["acme"], &["*"], &["*"], &["*"])]);
        assert!(id.is_authorized("acme", "x", "y", "z"));
        assert!(id.is_authorized("acme.us-east", "x", "y", "z"));
        assert!(id.is_authorized("acme.us-east.prod", "x", "y", "z"));
        assert!(!id.is_authorized("acme-corp", "x", "y", "z"));
        assert!(!id.is_authorized("other", "x", "y", "z"));
    }

    #[test]
    fn multiple_grants_union() {
        let id = identity_with(vec![
            grant(&["acme"], &["prod"], &["email"], &["*"]),
            grant(&["beta"], &["*"], &["*"], &["*"]),
        ]);
        assert!(id.is_authorized("acme", "prod", "email", "send"));
        assert!(!id.is_authorized("acme", "prod", "sms", "send"));
        assert!(id.is_authorized("beta", "anything", "anything", "anything"));
    }

    #[test]
    fn allowed_providers_returns_none_for_wildcard() {
        let id = identity_with(vec![grant(&["acme"], &["*"], &["*"], &["*"])]);
        assert_eq!(id.allowed_providers(), None);
    }

    #[test]
    fn allowed_providers_returns_sorted_unique_list() {
        let id = identity_with(vec![
            grant(&["*"], &["*"], &["email", "sms"], &["*"]),
            grant(&["*"], &["*"], &["email", "slack"], &["*"]),
        ]);
        let providers = id.allowed_providers().expect("non-wildcard");
        assert!(providers.contains(&"email"));
        assert!(providers.contains(&"sms"));
        assert!(providers.contains(&"slack"));
        assert_eq!(providers.len(), 3); // deduplicated
    }

    #[test]
    fn allowed_tenants_wildcard_returns_none() {
        let id = identity_with(vec![grant(&["*"], &["*"], &["*"], &["*"])]);
        assert_eq!(id.allowed_tenants(), None);
    }
}
