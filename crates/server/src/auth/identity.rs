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

    /// Check whether this caller has any grant covering the given
    /// `(tenant, namespace)` pair, ignoring provider/action scoping.
    ///
    /// Used by tenant-scoped CRUD endpoints (silences, quotas, retention)
    /// where provider and action-type scoping don't apply to the
    /// resource itself. Hierarchical tenant matching applies — a grant
    /// on `"acme"` covers `"acme.us-east"`.
    pub fn can_manage_scope(&self, tenant: &str, namespace: &str) -> bool {
        self.grants.iter().any(|g| {
            super::config::tenant_matches(&g.tenants, tenant)
                && (g.namespaces.iter().any(|n| n == "*")
                    || g.namespaces.iter().any(|n| n == namespace))
        })
    }

    /// Convert to the minimal `Caller` for audit threading.
    pub fn to_caller(&self) -> Caller {
        Caller {
            id: self.id.clone(),
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

    /// Resolve the tenant filter **and** authorization scope to apply for a
    /// query endpoint whose backend delegates filtering to the store/aggregator
    /// (audit, analytics, rule-coverage). Such a query must never run unscoped
    /// for a caller restricted to specific tenants, or it would return records
    /// from every tenant.
    ///
    /// The returned [`TenantQueryScope`] carries two complementary pieces:
    /// - `tenant`: an explicit single-tenant filter the caller requested
    ///   (validated to be within their grants), or `None`.
    /// - `scope`: the caller's hierarchical grant patterns. The backend
    ///   restricts results to tenants covered by one of these (`p` covers `p`
    ///   and `p.*`). Empty = unrestricted.
    ///
    /// Behavior:
    /// - Caller names a tenant → it must be covered by a grant
    ///   (hierarchically, via [`tenant_matches`], so a grant on `acme`
    ///   authorizes `acme.prod`). The exact filter fully bounds the query, so
    ///   `scope` is left empty — this keeps key-addressed backends (`DynamoDB`)
    ///   on their efficient exact-match path. `Err(NotGranted)` (→ 403) if not
    ///   covered.
    /// - Caller names nothing, wildcard tenant access → unrestricted
    ///   (`tenant = None`, empty `scope`).
    /// - Caller names nothing, scoped → `tenant = None` with `scope` set to the
    ///   caller's grant patterns. The backend returns the **union of the
    ///   granted subtrees** — multi-tenant callers aggregate across their
    ///   tenants without having to name one.
    ///
    /// [`tenant_matches`]: super::config::tenant_matches
    pub fn resolve_tenant_query_scope(
        &self,
        requested: Option<&str>,
    ) -> Result<TenantQueryScope, TenantFilterError> {
        match requested {
            // Named tenant: a grant must cover it hierarchically. `tenant_matches`
            // also returns true for a `*` grant, so wildcard callers are
            // authorized here. The exact equality filter fully bounds the query.
            Some(requested) => {
                if self
                    .grants
                    .iter()
                    .any(|g| super::config::tenant_matches(&g.tenants, requested))
                {
                    Ok(TenantQueryScope {
                        tenant: Some(requested.to_owned()),
                        scope: Vec::new(),
                    })
                } else {
                    Err(TenantFilterError::NotGranted(requested.to_owned()))
                }
            }
            // No tenant named: wildcard callers run unrestricted; a scoped
            // caller's grant patterns become the authorization scope so the
            // backend returns the union of their granted subtrees.
            None => match self.allowed_tenants() {
                None => Ok(TenantQueryScope::default()),
                Some(allowed) => Ok(TenantQueryScope {
                    tenant: None,
                    scope: allowed.into_iter().map(str::to_owned).collect(),
                }),
            },
        }
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
    /// Explicit single-tenant filter the caller requested, validated to be
    /// within their grants. `None` = no explicit narrowing.
    pub tenant: Option<String>,
    /// Hierarchical grant patterns bounding which tenants the caller may read.
    /// Empty = unrestricted (wildcard caller, or an explicit in-grant `tenant`
    /// already bounds the query). Fed into the backend query's `tenant_scope`.
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

    fn grant(tenants: &[&str], namespaces: &[&str], providers: &[&str], actions: &[&str]) -> Grant {
        Grant {
            tenants: tenants.iter().map(|s| (*s).to_string()).collect(),
            namespaces: namespaces.iter().map(|s| (*s).to_string()).collect(),
            providers: providers.iter().map(|s| (*s).to_string()).collect(),
            actions: actions.iter().map(|s| (*s).to_string()).collect(),
            agent_id: None,
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

    fn grant_with_agent(tenants: &[&str], namespaces: &[&str], agent_id: Option<&str>) -> Grant {
        Grant {
            tenants: tenants.iter().map(|s| (*s).to_string()).collect(),
            namespaces: namespaces.iter().map(|s| (*s).to_string()).collect(),
            providers: vec!["*".into()],
            actions: vec!["*".into()],
            agent_id: agent_id.map(str::to_string),
        }
    }

    #[test]
    fn bus_agent_id_none_when_no_grant_has_one() {
        let id = identity_with(vec![grant(&["acme"], &["agents"], &["*"], &["*"])]);
        assert_eq!(id.bus_agent_id_for_scope("acme", "agents"), Ok(None));
    }

    #[test]
    fn bus_agent_id_resolved_from_matching_scope() {
        let id = identity_with(vec![grant_with_agent(
            &["acme"],
            &["agents"],
            Some("planner-1"),
        )]);
        assert_eq!(
            id.bus_agent_id_for_scope("acme", "agents"),
            Ok(Some("planner-1")),
        );
    }

    #[test]
    fn bus_agent_id_does_not_bleed_across_scopes() {
        // Grant binds the caller to `planner-1` only under the
        // `agents` namespace. A bus call under a different namespace
        // should not get this identity.
        let id = identity_with(vec![grant_with_agent(
            &["acme"],
            &["agents"],
            Some("planner-1"),
        )]);
        assert_eq!(id.bus_agent_id_for_scope("acme", "ops"), Ok(None));
    }

    #[test]
    fn bus_agent_id_walks_multiple_grants() {
        // Same caller acts as `planner-1` under `agents` and as
        // `ops-bot` under `ops`. Each scope resolves to its own
        // identity; no cross-contamination.
        let id = identity_with(vec![
            grant_with_agent(&["acme"], &["agents"], Some("planner-1")),
            grant_with_agent(&["acme"], &["ops"], Some("ops-bot")),
        ]);
        assert_eq!(
            id.bus_agent_id_for_scope("acme", "agents"),
            Ok(Some("planner-1")),
        );
        assert_eq!(
            id.bus_agent_id_for_scope("acme", "ops"),
            Ok(Some("ops-bot")),
        );
    }

    #[test]
    fn bus_agent_id_duplicate_across_grants_resolves_cleanly() {
        // Two grants both bind to the same agent_id under the same
        // scope. That's redundant but not ambiguous — accept it
        // rather than reject.
        let id = identity_with(vec![
            grant_with_agent(&["acme"], &["agents"], Some("planner-1")),
            grant_with_agent(&["acme"], &["*"], Some("planner-1")),
        ]);
        assert_eq!(
            id.bus_agent_id_for_scope("acme", "agents"),
            Ok(Some("planner-1")),
        );
    }

    #[test]
    fn bus_agent_id_conflicting_grants_rejected() {
        // Two grants match the same scope but bind to *different*
        // agent ids. We refuse to guess which one to stamp.
        let id = identity_with(vec![
            grant_with_agent(&["acme"], &["agents"], Some("planner-1")),
            grant_with_agent(&["acme"], &["*"], Some("ops-bot")),
        ]);
        assert!(matches!(
            id.bus_agent_id_for_scope("acme", "agents"),
            Err(BusAgentIdResolutionError::ConflictingGrants { .. })
        ));
    }

    #[test]
    fn bus_agent_id_hierarchical_tenant_match() {
        // Tenant matching is hierarchical (a grant on `acme`
        // covers `acme.us-east`). The agent-id resolver should
        // honor that.
        let id = identity_with(vec![grant_with_agent(
            &["acme"],
            &["agents"],
            Some("planner-1"),
        )]);
        assert_eq!(
            id.bus_agent_id_for_scope("acme.us-east", "agents"),
            Ok(Some("planner-1")),
        );
    }

    // ---- resolve_tenant_query_scope --------------------------------------

    fn scope(tenant: Option<&str>, patterns: &[&str]) -> TenantQueryScope {
        TenantQueryScope {
            tenant: tenant.map(str::to_owned),
            scope: patterns.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn tenant_scope_wildcard_caller_is_unrestricted() {
        let id = identity_with(vec![grant(&["*"], &["*"], &["*"], &["*"])]);
        // No tenant named: unrestricted (no filter, empty scope → sees all).
        assert_eq!(id.resolve_tenant_query_scope(None), Ok(scope(None, &[])));
        // Named tenant: honored as an exact filter, no extra scope.
        assert_eq!(
            id.resolve_tenant_query_scope(Some("acme")),
            Ok(scope(Some("acme"), &[])),
        );
    }

    #[test]
    fn tenant_scope_single_tenant_caller_gets_subtree_scope() {
        let id = identity_with(vec![grant(&["acme"], &["*"], &["*"], &["*"])]);
        // No tenant named: scope to the granted pattern so the backend returns
        // the whole `acme` subtree (acme, acme.prod, …), not just exact `acme`.
        assert_eq!(
            id.resolve_tenant_query_scope(None),
            Ok(scope(None, &["acme"])),
        );
        // Naming the granted tenant pins it exactly (no extra scope needed).
        assert_eq!(
            id.resolve_tenant_query_scope(Some("acme")),
            Ok(scope(Some("acme"), &[])),
        );
    }

    #[test]
    fn tenant_scope_rejects_uncovered_named_tenant() {
        let id = identity_with(vec![grant(&["acme"], &["*"], &["*"], &["*"])]);
        assert_eq!(
            id.resolve_tenant_query_scope(Some("globex")),
            Err(TenantFilterError::NotGranted("globex".to_owned())),
        );
    }

    #[test]
    fn tenant_scope_allows_named_hierarchical_subtenant() {
        // A grant on `acme` authorizes naming a sub-tenant (a regional admin
        // querying `?tenant=acme.prod`), matching the hierarchical model.
        let id = identity_with(vec![grant(&["acme"], &["*"], &["*"], &["*"])]);
        assert_eq!(
            id.resolve_tenant_query_scope(Some("acme.prod")),
            Ok(scope(Some("acme.prod"), &[])),
        );
        // Segment boundary: `acme` must NOT cover the sibling `acmecorp`.
        assert_eq!(
            id.resolve_tenant_query_scope(Some("acmecorp")),
            Err(TenantFilterError::NotGranted("acmecorp".to_owned())),
        );
    }

    #[test]
    fn tenant_scope_multi_tenant_caller_without_filter_unions_grants() {
        // The former `Ambiguous` 400 case: a caller scoped to two tenants who
        // names none now aggregates across both granted subtrees via `scope`.
        let id = identity_with(vec![
            grant(&["acme"], &["*"], &["*"], &["*"]),
            grant(&["globex"], &["*"], &["*"], &["*"]),
        ]);
        let resolved = id
            .resolve_tenant_query_scope(None)
            .expect("multi-tenant caller is no longer rejected");
        assert_eq!(resolved.tenant, None);
        assert!(resolved.scope.contains(&"acme".to_owned()));
        assert!(resolved.scope.contains(&"globex".to_owned()));
    }

    #[test]
    fn tenant_scope_multi_tenant_caller_naming_a_tenant_pins_it() {
        let id = identity_with(vec![
            grant(&["acme"], &["*"], &["*"], &["*"]),
            grant(&["globex"], &["*"], &["*"], &["*"]),
        ]);
        // Naming a covered tenant pins the query to it (exact filter).
        assert_eq!(
            id.resolve_tenant_query_scope(Some("globex")),
            Ok(scope(Some("globex"), &[])),
        );
        // Naming an uncovered tenant is still forbidden.
        assert_eq!(
            id.resolve_tenant_query_scope(Some("initech")),
            Err(TenantFilterError::NotGranted("initech".to_owned())),
        );
    }
}
