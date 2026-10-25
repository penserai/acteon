use acteon_core::Caller;

use super::config::Grant;
use super::role::Role;

/// Rich server-side caller identity extracted from authentication.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// Exact context established by authentication, independent of grant breadth.
    pub scope: acteon_core::TenantContext,
    /// Caller identifier (username or API key name).
    pub id: String,
    /// Trusted credential-to-actor binding, independent of its display name.
    pub principal: Option<acteon_core::PrincipalIdentity>,
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
            scope: acteon_core::TenantContext::new("default", "default"),
            id: String::new(),
            principal: None,
            role: Role::Admin,
            grants: vec![Grant {
                tenants: vec!["*".to_owned()],
                namespaces: vec!["*".to_owned()],
                providers: vec!["*".to_owned()],
                actions: vec!["*".to_owned()],
                // Anonymous mode (auth disabled) is for local dev /
                // single-tenant deployments where there's no
                // multi-agent fleet. The bus handlers that need an
                // agent identity bound to the grant will reject
                // anonymous calls with a clear error.
                agent_id: None,
            }],
            auth_method: "anonymous".to_owned(),
        }
    }

    /// Check if this caller is authorized for a specific
    /// `(tenant, namespace, provider, action_type)` tuple.
    ///
    /// Grants authorize operations only inside the credential's exact context.
    pub fn is_authorized(
        &self,
        tenant: &str,
        namespace: &str,
        provider: &str,
        action_type: &str,
    ) -> bool {
        self.scope.matches(namespace, tenant)
            && self
                .grants
                .iter()
                .any(|g| g.matches(tenant, namespace, provider, action_type))
    }

    /// Exact tenant context used to bound data reads, even for wildcard grants.
    pub fn allowed_tenants(&self) -> Option<Vec<&str>> {
        Some(vec![self.scope.tenant.as_str()])
    }

    /// Exact namespace context used to bound data reads.
    pub fn allowed_namespaces(&self) -> Option<Vec<&str>> {
        Some(vec![self.scope.namespace.as_str()])
    }

    /// Return allowed providers, or `None` if the caller has wildcard provider access.
    pub fn allowed_providers(&self) -> Option<Vec<&str>> {
        let mut providers = Vec::new();
        for g in &self.grants {
            if !super::config::tenant_matches(&g.tenants, &self.scope.tenant)
                || !g
                    .namespaces
                    .iter()
                    .any(|n| n == "*" || n == &self.scope.namespace)
            {
                continue;
            }
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

    /// Check whether this caller has any grant covering the given
    /// `(tenant, namespace)` pair, ignoring provider/action scoping.
    ///
    /// Used by tenant-scoped CRUD endpoints (silences, quotas, retention)
    /// where provider and action-type scoping don't apply to the
    /// resource itself. The credential's exact context always bounds access.
    pub fn can_manage_scope(&self, tenant: &str, namespace: &str) -> bool {
        self.scope.matches(namespace, tenant)
            && self.grants.iter().any(|g| {
                super::config::tenant_matches(&g.tenants, tenant)
                    && (g.namespaces.iter().any(|n| n == "*")
                        || g.namespaces.iter().any(|n| n == namespace))
            })
    }

    /// Convert to the minimal `Caller` for audit threading.
    pub fn to_caller(&self) -> Caller {
        Caller {
            id: self.id.clone(),
            principal: self.principal.clone(),
            auth_method: self.auth_method.clone(),
        }
    }

    /// **Phase 10**: resolve the bus agent identity bound to this
    /// caller for the given `(tenant, namespace)` scope.
    ///
    /// Walks the caller's grants, finds those whose tenant +
    /// namespace match the scope, and pulls the `agent_id` from any
    /// grant that has one set. Returns:
    ///
    /// - `Ok(Some(agent_id))` — exactly one matching grant has an
    ///   `agent_id` set (or several grants agree on the same value).
    /// - `Ok(None)` — no matching grant has an `agent_id`. The
    ///   caller is authorized for this scope but not bound to any
    ///   bus identity; bus operations that need a sender (post
    ///   tool-call, append message on a private conversation, etc.)
    ///   will reject.
    /// - `Err(Conflict)` — multiple matching grants bind to
    ///   different `agent_id` values. This is an operator
    ///   misconfiguration; we refuse to guess which identity to
    ///   stamp.
    pub fn bus_agent_id_for_scope(
        &self,
        tenant: &str,
        namespace: &str,
    ) -> Result<Option<&str>, BusAgentIdResolutionError> {
        if !self.scope.matches(namespace, tenant) {
            return Ok(None);
        }
        let mut found: Option<&str> = None;
        for g in &self.grants {
            if !super::config::tenant_matches(&g.tenants, tenant) {
                continue;
            }
            // Namespace dimension: wildcard or exact, mirroring the
            // matching rules dispatch already uses.
            let ns_match = g.namespaces.iter().any(|n| n == "*" || n == namespace);
            if !ns_match {
                continue;
            }
            if let Some(id) = g.agent_id.as_deref() {
                match found {
                    None => found = Some(id),
                    Some(prev) if prev == id => {} // duplicate — fine
                    Some(prev) => {
                        return Err(BusAgentIdResolutionError::ConflictingGrants {
                            first: prev.to_string(),
                            second: id.to_string(),
                        });
                    }
                }
            }
        }
        Ok(found)
    }

    /// Always pin queries to the authenticated tenant. Hierarchical grants
    /// authorize permissions; they never widen a credential's data context.
    pub fn resolve_tenant_query_scope(
        &self,
        requested: Option<&str>,
    ) -> Result<TenantQueryScope, TenantFilterError> {
        if let Some(requested) = requested
            && requested != self.scope.tenant
        {
            return Err(TenantFilterError::NotGranted(requested.to_owned()));
        }
        Ok(TenantQueryScope {
            tenant: Some(self.scope.tenant.clone()),
            scope: Vec::new(),
        })
    }
}

/// Error returned by [`CallerIdentity::bus_agent_id_for_scope`] when
/// the caller's grants disagree about which agent identity to bind
/// for a given `(tenant, namespace)`. Operators should not have
/// overlapping grants with conflicting agent ids; the bus refuses
/// to guess.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum BusAgentIdResolutionError {
    #[error(
        "ambiguous bus agent identity for caller: grants bind to both '{first}' and '{second}'"
    )]
    ConflictingGrants { first: String, second: String },
}

/// The tenant filter and authorization scope resolved by
/// [`CallerIdentity::resolve_tenant_query_scope`] for a query endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TenantQueryScope {
    /// The credential's exact tenant, always populated by the HTTP boundary.
    pub tenant: Option<String>,
    /// Additional backend patterns; empty because `tenant` pins the query exactly.
    pub scope: Vec<String>,
}

/// Error returned by [`CallerIdentity::resolve_tenant_query_scope`] that the
/// caller must surface as an HTTP error instead of running the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantFilterError {
    /// The caller requested a tenant not covered by any of their grants.
    /// Map to `403 Forbidden`.
    NotGranted(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(
        tenant: &str,
        namespace: &str,
        provider: &str,
        action: &str,
        agent: Option<&str>,
    ) -> Grant {
        Grant {
            tenants: vec![tenant.into()],
            namespaces: vec![namespace.into()],
            providers: vec![provider.into()],
            actions: vec![action.into()],
            agent_id: agent.map(str::to_owned),
        }
    }
    fn identity(grants: Vec<Grant>) -> CallerIdentity {
        CallerIdentity {
            scope: acteon_core::TenantContext::new("prod", "acme"),
            id: "test".into(),
            principal: None,
            role: Role::Admin,
            grants,
            auth_method: "test".into(),
        }
    }

    #[test]
    fn anonymous_access_is_fixed_to_the_development_context() {
        let caller = CallerIdentity::anonymous();
        assert!(caller.is_authorized("default", "default", "any", "any"));
        assert!(!caller.is_authorized("other", "default", "any", "any"));
        assert!(!caller.is_authorized("default", "other", "any", "any"));
    }

    #[test]
    fn wildcard_and_hierarchical_grants_do_not_expand_context() {
        for tenant in ["*", "acme"] {
            let caller = identity(vec![grant(tenant, "*", "*", "*", None)]);
            assert!(caller.is_authorized("acme", "prod", "email", "send"));
            for (t, n) in [("acme.child", "prod"), ("other", "prod"), ("acme", "stage")] {
                assert!(!caller.is_authorized(t, n, "email", "send"));
                assert!(!caller.can_manage_scope(t, n));
            }
            assert_eq!(caller.allowed_tenants(), Some(vec!["acme"]));
            assert_eq!(caller.allowed_namespaces(), Some(vec!["prod"]));
        }
    }

    #[test]
    fn operation_grants_remain_correlated_inside_context() {
        let caller = identity(vec![
            grant("acme", "prod", "email", "send", None),
            grant("acme", "prod", "sms", "receive", None),
            grant("other", "*", "*", "*", None),
        ]);
        assert!(caller.is_authorized("acme", "prod", "email", "send"));
        assert!(caller.is_authorized("acme", "prod", "sms", "receive"));
        assert!(!caller.is_authorized("acme", "prod", "sms", "send"));
        assert!(!caller.is_authorized("acme", "prod", "email", "receive"));
        assert!(!caller.is_authorized("other", "prod", "email", "send"));
        assert_eq!(caller.allowed_providers(), Some(vec!["email", "sms"]));
    }

    #[test]
    fn wildcard_provider_in_context_is_unrestricted_only_for_providers() {
        let caller = identity(vec![grant("acme", "prod", "*", "send", None)]);
        assert_eq!(caller.allowed_providers(), None);
        assert!(!caller.is_authorized("acme", "prod", "email", "delete"));
    }

    #[test]
    fn query_is_pinned_even_for_multi_tenant_grants() {
        let caller = identity(vec![
            grant("*", "*", "*", "*", None),
            grant("globex", "*", "*", "*", None),
        ]);
        for requested in [None, Some("acme")] {
            assert_eq!(
                caller.resolve_tenant_query_scope(requested),
                Ok(TenantQueryScope {
                    tenant: Some("acme".into()),
                    scope: vec![]
                })
            );
        }
        for requested in ["globex", "acme.prod", "acmecorp", "*"] {
            assert_eq!(
                caller.resolve_tenant_query_scope(Some(requested)),
                Err(TenantFilterError::NotGranted(requested.into()))
            );
        }
    }

    #[test]
    fn agent_binding_uses_only_the_authenticated_context() {
        let caller = identity(vec![
            grant("acme", "prod", "*", "*", Some("planner")),
            grant("acme", "stage", "*", "*", Some("test-agent")),
        ]);
        assert_eq!(
            caller.bus_agent_id_for_scope("acme", "prod"),
            Ok(Some("planner"))
        );
        assert_eq!(caller.bus_agent_id_for_scope("acme", "stage"), Ok(None));
        assert_eq!(
            caller.bus_agent_id_for_scope("acme.child", "prod"),
            Ok(None)
        );
        assert_eq!(
            identity(vec![grant("*", "*", "*", "*", None)]).bus_agent_id_for_scope("acme", "prod"),
            Ok(None)
        );
    }

    #[test]
    fn duplicate_agent_bindings_are_allowed_but_conflicts_are_rejected() {
        let mut caller = identity(vec![
            grant("acme", "prod", "*", "*", Some("planner")),
            grant("*", "*", "*", "*", Some("planner")),
        ]);
        assert_eq!(
            caller.bus_agent_id_for_scope("acme", "prod"),
            Ok(Some("planner"))
        );
        caller.grants[1].agent_id = Some("different".into());
        assert!(matches!(
            caller.bus_agent_id_for_scope("acme", "prod"),
            Err(BusAgentIdResolutionError::ConflictingGrants { .. })
        ));
    }
}
