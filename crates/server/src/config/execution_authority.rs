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
    #[serde(default)]
    pub agent_driver: AgentServiceDriverConfig,
    pub scopes: Vec<ExecutionScopeConfig>,
}

/// Host scheduling controls, separate from agent permits and service identity.
/// Disabling the driver parks accepted work; it never certifies cancellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentServiceDriverConfig {
    pub enabled: bool,
    pub poll_interval_ms: u64,
    pub max_parallel: usize,
    pub scan_batch_size: usize,
}
impl Default for AgentServiceDriverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 500,
            max_parallel: 4,
            scan_batch_size: 64,
        }
    }
}
impl AgentServiceDriverConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(50..=60_000).contains(&self.poll_interval_ms)
            || !(1..=32).contains(&self.max_parallel)
            || !(1..=256).contains(&self.scan_batch_size)
        {
            return Err("invalid bounded agent driver configuration".into());
        }
        Ok(())
    }
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
    /// Retain verified evidence without any live execution or write-management grant.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub history_only: bool,
    /// Retained evidence management with qualified finality acceptance, no live work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reconciliation_only: bool,
    pub publisher: PrincipalIdentity,
    pub subjects: Vec<PrincipalIdentity>,
    pub routes: Vec<ExecutionRouteConfig>,
    /// Explicit chain start bounds. Provider routes never imply chain rights.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<ExecutionChainDeclaration>,
    /// Explicit individual-agent runtime and service delegation declarations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_services: Vec<crate::execution_authority::agent_services::AgentServiceDeclaration>,
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

/// Chain names and actors explicitly reviewed by the operator. All provider
/// effects still require their own independent credential and permit bounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionChainDeclaration {
    pub name: String,
    pub subjects: Vec<PrincipalIdentity>,
}
impl ExecutionChainDeclaration {
    pub(crate) fn effect(&self, namespace: &str, tenant: &str) -> Result<AcceptedEffect, String> {
        Ok(AcceptedEffect {
            operation: "chain.start".into(),
            resources: vec![
                ResourceRef::new(ResourceKind::Chain, namespace, tenant, &self.name)
                    .map_err(|_| "invalid concrete chain declaration")?,
            ],
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPermitDeclaration {
    pub id: String,
    pub revision: u64,
    pub subject: PrincipalIdentity,
    pub routes: Vec<ExecutionRouteConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
// Independent additive permissions retain the established wire/config contract.
#[allow(clippy::struct_excessive_bools)]
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
    /// Explicit access to retained provider evidence for declared subjects.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_read_history: bool,
    /// Independent permission to accept qualified finality for retained work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_reconcile: bool,
    /// Exact independent finality-management footprint, including enclosing
    /// resources for descendants. Routes alone do not authorize reconciliation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconciliation_resources: Vec<ResourceRef>,
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
        self.agent_driver.validate()?;
        if self.scopes.is_empty() || self.scopes.len() > 128 {
            return Err("execution authority requires 1..128 declared scopes".into());
        }
        let mut scopes = BTreeSet::new();
        for scope in &self.scopes {
            scope.validate_chains()?;
            scope.validate_agents()?;
            scope.validate_permits()?;
            scope.validate_managers()?;
            scope.validate_history_only()?;
            scope.validate_reconciliation_only()?;
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
                || (routes.is_empty() && !scope.retained_only())
                || routes.len() > 128
                || routes.len() != scope.routes.len()
                || scope.historical_effects.len() + routes.len() + scope.chains.len() > 128
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
    pub(crate) fn retained_only(&self) -> bool {
        self.history_only || self.reconciliation_only
    }

    fn validate_reconciliation_only(&self) -> Result<(), String> {
        if self.reconciliation_only
            && (self.history_only
                || self.bootstrap
                || !self.routes.is_empty()
                || !self.chains.is_empty()
                || !self.agent_services.is_empty()
                || !self.permits.is_empty()
                || self.historical_effects.is_empty()
                || self.managers.is_empty()
                || self.managers.iter().any(|manager| {
                    (!manager.can_read_history && !manager.can_reconcile)
                        || manager.can_issue_permits
                        || manager.can_intervene
                        || manager.workforce.is_some()
                }))
        {
            return Err("reconciliation-only scopes require retained effects and evidence managers, with no bootstrap or live work".into());
        }
        Ok(())
    }

    fn validate_history_only(&self) -> Result<(), String> {
        if self.history_only
            && (self.bootstrap
                || !self.routes.is_empty()
                || !self.chains.is_empty()
                || !self.agent_services.is_empty()
                || !self.permits.is_empty()
                || self.historical_effects.is_empty()
                || self.managers.is_empty()
                || self.managers.iter().any(|manager| {
                    !manager.can_read_history
                        || manager.can_issue_permits
                        || manager.can_intervene
                        || manager.can_reconcile
                        || manager.workforce.is_some()
                }))
        {
            return Err("history-only scopes require retained effects and read-only managers, with no bootstrap or live work".into());
        }
        Ok(())
    }

    fn validate_agents(&self) -> Result<(), String> {
        let mut agents = BTreeSet::new();
        let mut grants = BTreeSet::new();
        if self.agent_services.len() > 128 {
            return Err("too many declared agent services".into());
        }
        for service in &self.agent_services {
            service.validate(self)?;
            if !agents.insert(&service.card.agent_id)
                || service.grants.iter().any(|grant| !grants.insert(&grant.id))
            {
                return Err("agent service and grant identities must be unique".into());
            }
        }
        Ok(())
    }

    fn validate_chains(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        if self.chains.len() > 128 {
            return Err("too many declared chains".into());
        }
        for chain in &self.chains {
            chain.effect(&self.namespace, &self.tenant)?;
            if !names.insert(&chain.name)
                || chain.name.contains('*')
                || chain.subjects.is_empty()
                || chain.subjects.len() > 16
                || chain
                    .subjects
                    .iter()
                    .map(PrincipalIdentity::id)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != chain.subjects.len()
                || chain
                    .subjects
                    .iter()
                    .any(|subject| !self.subjects.contains(subject))
            {
                return Err("chain admission exceeds independently declared bounds".into());
            }
        }
        Ok(())
    }
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
                    && !manager.can_read_history
                    && !manager.can_reconcile
                    && manager.workforce.is_none())
                || manager.reconciliation_resources.len() > 128
                || manager
                    .reconciliation_resources
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != manager.reconciliation_resources.len()
                || manager
                    .reconciliation_resources
                    .iter()
                    .any(|r| r.namespace() != self.namespace || r.tenant() != self.tenant)
                || manager.can_reconcile == manager.reconciliation_resources.is_empty()
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
                || (permit.routes.is_empty()
                    && permit.chains.is_empty()
                    && permit.agents.is_empty())
                || permit.routes.len() + permit.chains.len() + permit.agents.len() > 128
                || permit.chains.iter().collect::<BTreeSet<_>>().len() != permit.chains.len()
                || permit.chains.iter().any(|name| {
                    !self.chains.iter().any(|chain| {
                        chain.name == *name && chain.subjects.contains(&permit.subject)
                    })
                })
                || permit.agents.iter().collect::<BTreeSet<_>>().len() != permit.agents.len()
                || permit.agents.iter().any(|id| {
                    !self.agent_services.iter().any(|service| {
                        service.card.agent_id == *id
                            && service
                                .grants
                                .iter()
                                .any(|grant| grant.source == permit.subject)
                    })
                })
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
