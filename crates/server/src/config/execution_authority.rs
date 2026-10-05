//! Deployment declarations for the production execution-authority integration.
//! These are trusted operator inputs, never an HTTP authorization payload.
use std::collections::BTreeSet;

use acteon_core::{PrincipalIdentity, ResourceKind, ResourceRef, TeamRef};
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
    /// Explicit operator-issued permits. Omitting an entry does not revoke it.
    #[serde(default)]
    pub permits: Vec<ExecutionPermitDeclaration>,
    /// Independent authenticated management rights; omitted means no public management.
    #[serde(default)]
    pub managers: Vec<ExecutionManagerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPermitDeclaration {
    pub id: String,
    pub revision: u64,
    pub subject: PrincipalIdentity,
    pub routes: Vec<ExecutionRouteConfig>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionManagerConfig {
    pub principal: PrincipalIdentity,
    pub subjects: Vec<PrincipalIdentity>,
    pub routes: Vec<ExecutionRouteConfig>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
    #[serde(default)]
    pub can_issue_permits: bool,
    #[serde(default)]
    pub can_intervene: bool,
    /// Omitted means no workforce management, even for a permit issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workforce: Option<WorkforceManagerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkforceManagerConfig {
    pub teams: Vec<TeamRef>,
    pub job_classes: Vec<String>,
    #[serde(default)]
    pub can_manage_roster: bool,
    #[serde(default)]
    pub can_issue_mandates: bool,
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
            scope.validate_permits()?;
            scope.validate_managers()?;
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

impl ExecutionScopeConfig {
    fn validate_managers(&self) -> Result<(), String> {
        if self.managers.len() > 16 {
            return Err("too many execution managers".into());
        }
        let mut actors = BTreeSet::new();
        for manager in &self.managers {
            self.validate_workforce_manager(manager)?;
            if !actors.insert(manager.principal.id())
                || !self.subjects.contains(&manager.principal)
                || manager.subjects.is_empty()
                || manager.subjects.len() > 16
                || manager
                    .subjects
                    .iter()
                    .map(PrincipalIdentity::id)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != manager.subjects.len()
                || manager.subjects.iter().any(|s| !self.subjects.contains(s))
                || manager.routes.len() > 128
                || manager.routes.iter().collect::<BTreeSet<_>>().len() != manager.routes.len()
                || manager.routes.iter().any(|r| !self.routes.contains(r))
                || (!manager.can_issue_permits
                    && !manager.can_intervene
                    && manager.workforce.is_none())
                || (manager.can_issue_permits && manager.routes.is_empty())
                || manager.valid_from_ms < self.valid_from_ms
                || manager.limits.deadline_ms <= manager.valid_from_ms
                || manager.limits.deadline_ms > self.credential_limits.deadline_ms
                || manager.limits.max_units == 0
                || manager.limits.max_units > self.credential_limits.max_units
                || manager.limits.max_concurrent == 0
                || manager.limits.max_concurrent > self.credential_limits.max_concurrent
            {
                return Err("execution management exceeds independently declared bounds".into());
            }
        }
        Ok(())
    }

    fn validate_workforce_manager(&self, manager: &ExecutionManagerConfig) -> Result<(), String> {
        let Some(workforce) = &manager.workforce else {
            return Ok(());
        };
        if workforce.teams.len() > 128
            || workforce.teams.iter().collect::<BTreeSet<_>>().len() != workforce.teams.len()
            || workforce.teams.iter().any(|t| t.tenant() != self.tenant)
            || workforce.job_classes.len() > 128
            || workforce.job_classes.iter().collect::<BTreeSet<_>>().len()
                != workforce.job_classes.len()
            || workforce
                .job_classes
                .iter()
                .any(|c| !manager.routes.iter().any(|r| r.action_type == *c))
            || (!workforce.can_manage_roster
                && !workforce.can_issue_mandates
                && !manager.can_issue_permits)
            || ((workforce.can_issue_mandates || manager.can_issue_permits)
                && workforce.job_classes.is_empty())
        {
            return Err("workforce management exceeds independently declared bounds".into());
        }
        Ok(())
    }

    fn validate_permits(&self) -> Result<(), String> {
        let mut permit_ids = BTreeSet::new();
        if self.permits.len() > 128 {
            return Err("too many deployment permits".into());
        }
        for permit in &self.permits {
            ResourceRef::new(
                ResourceKind::Action,
                &self.namespace,
                &self.tenant,
                &permit.id,
            )
            .map_err(|_| "invalid deployment permit ID")?;
            if !permit_ids.insert(&permit.id)
                || permit.revision == 0
                || !self.subjects.contains(&permit.subject)
                || permit.routes.is_empty()
                || permit.routes.len() > 128
                || permit.routes.iter().any(|r| !self.routes.contains(r))
                || permit.routes.iter().collect::<BTreeSet<_>>().len() != permit.routes.len()
                || permit.valid_from_ms < self.valid_from_ms
                || permit.limits.max_units == 0
                || permit.limits.max_concurrent == 0
                || permit.limits.max_units > self.credential_limits.max_units
                || permit.limits.max_concurrent > self.credential_limits.max_concurrent
                || permit.limits.deadline_ms > self.credential_limits.deadline_ms
                || permit.limits.deadline_ms <= permit.valid_from_ms
            {
                return Err("deployment permit exceeds independently declared scope bounds".into());
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
    #[test]
    fn workforce_rights_are_explicit_scoped_and_canonical() {
        let mut configuration = declaration();
        let scope = &mut configuration.scopes[0];
        let mut manager: ExecutionManagerConfig = serde_json::from_value(serde_json::json!({
            "principal":scope.subjects[0], "subjects":scope.subjects,"routes":scope.routes,
            "valid_from_ms":scope.valid_from_ms, "limits":scope.credential_limits,
            "can_issue_permits":true,"can_intervene":false
        }))
        .unwrap();
        assert!(
            serde_json::to_value(&manager)
                .unwrap()
                .get("workforce")
                .is_none()
        );
        manager.workforce = Some(WorkforceManagerConfig {
            teams: vec![TeamRef::new("prod", "acme", "reliability").unwrap()],
            job_classes: vec!["execute".into()],
            can_manage_roster: true,
            can_issue_mandates: true,
        });
        scope.managers.push(manager);
        configuration
            .validate(("auth-control", "deployment"))
            .unwrap();
        for variation in 0..4 {
            let mut invalid = configuration.clone();
            let workforce = invalid.scopes[0].managers[0].workforce.as_mut().unwrap();
            match variation {
                0 => workforce.teams[0] = TeamRef::new("prod", "foreign", "reliability").unwrap(),
                1 => workforce.job_classes = vec!["undeclared".into()],
                2 => workforce.teams.push(workforce.teams[0].clone()),
                _ => workforce.job_classes.clear(),
            }
            assert!(invalid.validate(("auth-control", "deployment")).is_err());
        }
        let manager = &mut configuration.scopes[0].managers[0];
        manager.can_issue_permits = false;
        manager.routes.clear();
        manager.workforce.as_mut().unwrap().can_issue_mandates = false;
        manager.workforce.as_mut().unwrap().job_classes.clear();
        configuration
            .validate(("auth-control", "deployment"))
            .unwrap();
    }
}
