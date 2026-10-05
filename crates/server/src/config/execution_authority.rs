//! Deployment declarations for the production execution-authority integration.
//! These are trusted operator inputs, never an HTTP authorization payload.
use std::collections::BTreeSet;

use acteon_core::{PrincipalIdentity, ResourceKind, ResourceRef};
use acteon_governance::{RootBudgetLimits, context::AcceptedEffect};
use serde::{Deserialize, Serialize};

/// Complete deployment scope manifest. Runtime installation must publish every
/// declared scope before exposing authentication or starting its watcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorityConfig {
    pub scopes: Vec<ExecutionScopeConfig>,
}

/// Independent publication bounds are explicit, rather than inferred from
/// authentication grants or previous state. Absolute validity timestamps keep
/// identical declarations deterministic across replicas and restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScopeConfig {
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub bootstrap: bool,
    pub publisher: PrincipalIdentity,
    pub subjects: Vec<PrincipalIdentity>,
    pub routes: Vec<ExecutionRouteConfig>,
    #[serde(default)]
    pub historical_effects: Vec<AcceptedEffect>,
    pub valid_from_ms: i64,
    pub credential_limits: RootBudgetLimits,
    pub root_max_units: u64,
    pub root_max_concurrent: u64,
    pub root_lifetime_ms: u64,
}

/// A concrete primary action on an actual registered provider. Wildcard routes
/// do not qualify dynamic destinations or unreviewed auxiliary effects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRouteConfig {
    pub provider: String,
    pub action_type: String,
}

impl ExecutionAuthorityConfig {
    pub fn validate(&self, control_scope: (&str, &str)) -> Result<(), String> {
        if self.scopes.is_empty() || self.scopes.len() > 128 {
            return Err("execution authority requires 1..128 declared scopes".into());
        }
        let mut scopes = BTreeSet::new();
        for scope in &self.scopes {
            if (scope.namespace.as_str(), scope.tenant.as_str()) == control_scope
                || !scopes.insert((&scope.namespace, &scope.tenant))
            {
                return Err(
                    "execution scopes must be unique and separate from auth control".into(),
                );
            }
            ResourceRef::new(
                ResourceKind::Endpoint,
                &scope.namespace,
                &scope.tenant,
                "scope-validation",
            )
            .map_err(|_| "invalid execution scope identity")?;
            let subjects: BTreeSet<_> = scope.subjects.iter().map(PrincipalIdentity::id).collect();
            let routes: BTreeSet<_> = scope.routes.iter().collect();
            if subjects.is_empty()
                || subjects.len() > 16
                || subjects.len() != scope.subjects.len()
                || routes.is_empty()
                || routes.len() > 128
                || routes.len() != scope.routes.len()
                || scope.historical_effects.len() + routes.len() > 128
                || scope.valid_from_ms < 0
                || scope.credential_limits.deadline_ms <= scope.valid_from_ms
                || scope.credential_limits.max_units == 0
                || scope.credential_limits.max_concurrent == 0
                || scope.root_max_units == 0
                || scope.root_max_units > scope.credential_limits.max_units
                || scope.root_max_concurrent == 0
                || scope.root_max_concurrent > scope.credential_limits.max_concurrent
                || scope.root_lifetime_ms == 0
                || i64::try_from(scope.root_lifetime_ms).is_err()
            {
                return Err("invalid execution publication or root allocation bounds".into());
            }
            for route in &scope.routes {
                for (kind, value) in [
                    (ResourceKind::Provider, &route.provider),
                    (ResourceKind::Action, &route.action_type),
                ] {
                    ResourceRef::new(kind, &scope.namespace, &scope.tenant, value)
                        .map_err(|_| "invalid concrete execution route")?;
                    if value.contains('*') {
                        return Err("execution qualification requires concrete routes".into());
                    }
                }
            }
            if scope
                .historical_effects
                .iter()
                .flat_map(|e| &e.resources)
                .any(|r| r.namespace() != scope.namespace || r.tenant() != scope.tenant)
            {
                return Err("historical publication effects must belong to their scope".into());
            }
            if !scope.historical_effects.is_empty() {
                acteon_governance::permit::PermitIssuanceCeiling {
                    issuer: scope.publisher.clone(),
                    subjects: scope.subjects.clone(),
                    effects: scope.historical_effects.clone(),
                    valid_from_ms: scope.valid_from_ms,
                    limits: scope.credential_limits.clone(),
                }
                .validate()
                .map_err(|_| "invalid historical publication ceiling")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration() -> ExecutionAuthorityConfig {
        serde_json::from_value(serde_json::json!({"scopes": [{
            "namespace": "prod", "tenant": "acme", "bootstrap": true,
            "publisher": {"id": "execution-publisher", "kind": "system"},
            "subjects": [{"id": "agent/maya", "kind": "agent"}],
            "routes": [{"provider": "incident", "action_type": "execute"}],
            "valid_from_ms": 0,
            "credential_limits": {"max_units": 5, "max_concurrent": 2, "deadline_ms": 4_102_444_800_000_i64},
            "root_max_units": 5, "root_max_concurrent": 1, "root_lifetime_ms": 60000
        }]})).unwrap()
    }

    #[test]
    fn deployment_bounds_and_control_isolation_are_explicit() {
        let config = declaration();
        config.validate(("auth-control", "deployment")).unwrap();
        assert!(config.validate(("prod", "acme")).is_err());
        let mut duplicate = config.clone();
        duplicate.scopes.push(config.scopes[0].clone());
        assert!(duplicate.validate(("auth-control", "deployment")).is_err());
        for variation in 0..4 {
            let mut changed = config.clone();
            match variation {
                0 => changed.scopes[0].root_max_units = 6,
                1 => changed.scopes[0].root_lifetime_ms = 0,
                2 => changed.scopes[0].routes[0].provider = "*".into(),
                _ => changed.scopes[0]
                    .subjects
                    .push(config.scopes[0].subjects[0].clone()),
            }
            assert!(changed.validate(("auth-control", "deployment")).is_err());
        }
    }

    #[test]
    fn historical_cleanup_authority_cannot_cross_scopes_or_use_invalid_effects() {
        let mut config = declaration();
        config.scopes[0].historical_effects.push(AcceptedEffect {
            operation: "provider.execute".into(),
            resources: vec![
                ResourceRef::new(ResourceKind::Endpoint, "other", "acme", "incident").unwrap(),
            ],
        });
        assert!(config.validate(("auth-control", "deployment")).is_err());
        config.scopes[0].historical_effects[0].resources.clear();
        assert!(config.validate(("auth-control", "deployment")).is_err());
        config.scopes[0].historical_effects[0].resources.push(
            ResourceRef::new(ResourceKind::Endpoint, "prod", "acme", "retired-endpoint").unwrap(),
        );
        config.validate(("auth-control", "deployment")).unwrap();
        config.scopes[0].historical_effects[0].operation.clear();
        assert!(config.validate(("auth-control", "deployment")).is_err());
    }
}
