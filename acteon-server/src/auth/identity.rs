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
                actions: vec!["*".to_owned()],
            }],
            auth_method: "anonymous".to_owned(),
        }
    }

    /// Check if this caller is authorized for a specific (tenant, namespace, `action_type`).
    pub fn is_authorized(&self, tenant: &str, namespace: &str, action_type: &str) -> bool {
        self.grants.iter().any(|g| {
            (g.tenants.iter().any(|t| t == "*") || g.tenants.iter().any(|t| t == tenant))
                && (g.namespaces.iter().any(|n| n == "*")
                    || g.namespaces.iter().any(|n| n == namespace))
                && (g.actions.iter().any(|a| a == "*")
                    || g.actions.iter().any(|a| a == action_type))
        })
    }

    /// Return the set of (tenant, namespace) pairs this caller has access to,
    /// for filtering audit queries. Returns `None` if the caller has wildcard
    /// access to all tenants.
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

    /// Convert to the minimal `Caller` for audit threading.
    pub fn to_caller(&self) -> Caller {
        Caller {
            id: self.id.clone(),
            auth_method: self.auth_method.clone(),
        }
    }
}
