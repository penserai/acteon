//! Deployment declarations for the production execution-authority integration.
//! These are trusted operator inputs, never an HTTP authorization payload.
use std::collections::BTreeSet;

use acteon_core::{PrincipalIdentity, ResourceKind, ResourceRef, TeamRef};
use acteon_governance::{
    RootBudgetLimits,
    context::AcceptedEffect,
    federation::{FederationTrust, FederationTrustIssuanceCeiling},
};
use serde::{Deserialize, Serialize};

/// Complete deployment scope manifest. Runtime installation must publish every
/// declared scope before exposing authentication or starting its watcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorityConfig {
    #[serde(default)]
    pub agent_driver: AgentServiceDriverConfig,
    #[serde(default)]
    pub peer_transport: AgentPeerTransportConfig,
    /// Host-installed authorization verifiers. Service declarations select an
    /// exact ID and revision; requests and model output cannot select one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authorization_verifiers: Vec<TaskAuthorizationHttpVerifierConfig>,
    pub scopes: Vec<ExecutionScopeConfig>,
}

/// Guarded HTTP trust adapter for resolving `AuthRequired` task challenges.
/// The bearer credential is loaded from `credential_env` by the host and is
/// never persisted in task state or exposed to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAuthorizationHttpVerifierConfig {
    pub id: String,
    pub revision: u64,
    pub endpoint: String,
    pub credential_env: String,
    #[serde(default = "default_authorization_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_hosts: Vec<String>,
}

fn default_authorization_timeout_ms() -> u64 {
    5_000
}

impl TaskAuthorizationHttpVerifierConfig {
    pub fn validate(&self) -> Result<(), String> {
        let env = self.credential_env.as_bytes();
        let hosts = self.internal_hosts.iter().collect::<BTreeSet<_>>();
        if self.id.is_empty()
            || self.id.len() > 120
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            || self.revision == 0
            || self.endpoint.len() > 2_048
            || env.is_empty()
            || env.len() > 128
            || !env[0].is_ascii_alphabetic()
            || !env.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
            || !(100..=120_000).contains(&self.timeout_ms)
            || self.internal_hosts.len() > 64
            || hosts.len() != self.internal_hosts.len()
            || self.internal_hosts.iter().any(|host| {
                host.is_empty()
                    || host.len() > 253
                    || host.trim() != host
                    || host.contains('/')
                    || host.contains('@')
                    || host.chars().any(char::is_control)
            })
        {
            return Err("invalid bounded task authorization verifier configuration".into());
        }
        acteon_http::OutboundPolicy {
            internal_hosts: self.internal_hosts.clone(),
        }
        .validate_url(&self.endpoint)
        .map_err(|_| "task authorization verifier endpoint refused by outbound policy")?;
        Ok(())
    }
}

/// Host-owned agent-to-agent delivery controls. Credentials and exact endpoints
/// remain on qualified service declarations and never enter invocation input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentPeerTransportConfig {
    pub enabled: bool,
    pub timeout_ms: u64,
    pub adapter_revision: String,
    pub internal_hosts: Vec<String>,
}
impl Default for AgentPeerTransportConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 10_000,
            adapter_revision: "acteon-peer-http-v1".into(),
            internal_hosts: Vec::new(),
        }
    }
}
impl AgentPeerTransportConfig {
    pub fn validate(&self) -> Result<(), String> {
        let hosts = self.internal_hosts.iter().collect::<BTreeSet<_>>();
        if !(100..=120_000).contains(&self.timeout_ms)
            || self.adapter_revision.is_empty()
            || self.adapter_revision.len() > 120
            || self.adapter_revision.trim() != self.adapter_revision
            || self.adapter_revision.chars().any(char::is_control)
            || self.internal_hosts.len() > 64
            || hosts.len() != self.internal_hosts.len()
            || self.internal_hosts.iter().any(|host| {
                host.is_empty()
                    || host.len() > 253
                    || host.trim() != host
                    || host.contains('/')
                    || host.contains('@')
                    || host.chars().any(char::is_control)
            })
        {
            return Err("invalid bounded agent peer transport configuration".into());
        }
        Ok(())
    }
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
    /// Exact prior service bindings available only to already accepted work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_agent_services:
        Vec<crate::execution_authority::agent_services::RetainedAgentServiceDeclaration>,
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
    /// Cross-domain trust roots and authenticated peer bindings are installed
    /// by the host. An absent block disables federation for this scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation: Option<ExecutionFederationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionFederationConfig {
    pub local_audience: String,
    /// Complete exact trust revisions a manager may publish or revoke.
    pub trusts: Vec<FederationTrust>,
    /// Foreign domains mapped to local private-authentication principals.
    pub peers: Vec<FederationPeerConfig>,
    /// Outbound profiles are exact source-to-remote-service bindings. Secrets
    /// are referenced by environment name and loaded only by runtime install.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<FederationExportConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPeerConfig {
    pub foreign_domain: String,
    pub authentication_principal: PrincipalIdentity,
    pub trust_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationExportConfig {
    pub id: String,
    pub source_agent_id: String,
    pub remote_origin: String,
    pub remote_audience: String,
    pub trust_id: String,
    pub trust_revision: u64,
    pub key_id: String,
    pub verifying_key: String,
    pub signer_key_env: String,
    pub issuer_epoch: u64,
    pub credential_env: String,
    pub grant: acteon_governance::federation::FederationGrantCeiling,
    #[serde(default = "default_federation_export_lifetime_ms")]
    pub max_lifetime_ms: i64,
    #[serde(default = "default_federation_export_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_hosts: Vec<String>,
}

fn default_federation_export_lifetime_ms() -> i64 {
    300_000
}

fn default_federation_export_timeout_ms() -> u64 {
    10_000
}

impl FederationExportConfig {
    pub(crate) fn local_effect(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<AcceptedEffect, String> {
        Ok(AcceptedEffect {
            operation: "federation.export".into(),
            resources: vec![
                ResourceRef::new(ResourceKind::ExternalService, namespace, tenant, &self.id)
                    .map_err(|_| "invalid federation export identity")?,
            ],
        })
    }

    fn validate(&self, scope: &ExecutionScopeConfig, local_audience: &str) -> Result<(), String> {
        let source = scope
            .agent_services
            .iter()
            .find(|agent| agent.card.agent_id == self.source_agent_id)
            .ok_or("federation export source service is not declared")?;
        if source.principal != self.grant.source
            || self.id.is_empty()
            || self.id.len() > 120
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            || !valid_env_name(&self.signer_key_env)
            || !valid_env_name(&self.credential_env)
            || !(100..=120_000).contains(&self.timeout_ms)
            || self.internal_hosts.len() > 64
            || self.internal_hosts.iter().collect::<BTreeSet<_>>().len()
                != self.internal_hosts.len()
        {
            return Err("invalid outbound federation profile".into());
        }
        acteon_http::OutboundPolicy {
            internal_hosts: self.internal_hosts.clone(),
        }
        .validate_url(&self.remote_origin)
        .map_err(|_| "federation export origin refused by outbound policy")?;
        acteon_executor::federation_export::QualifiedFederationExport::new_trusted(
            self.remote_origin.clone(),
            local_audience.to_owned(),
            self.remote_audience.clone(),
            self.trust_id.clone(),
            self.trust_revision,
            self.key_id.clone(),
            self.verifying_key.clone(),
            self.issuer_epoch,
            self.grant.target.clone(),
            self.grant.agent_resource.clone(),
            self.grant.binding_digest.clone(),
            self.grant.skill.clone(),
            self.grant.ingress_effect.clone(),
            self.grant.effects.clone(),
            self.grant.limits.clone(),
            self.grant.max_depth,
            self.max_lifetime_ms,
            self.local_effect(&scope.namespace, &scope.tenant)?,
        )
        .map_err(|_| "invalid qualified federation export")?;
        Ok(())
    }
}

fn valid_env_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphabetic()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub federation_exports: Vec<String>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
// Independent additive permissions retain the established wire/config contract.
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutionManagerConfig {
    /// Exact independent registry/control footprint; routes do not imply agent management.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
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
    /// Exact trust IDs this manager may publish or terminally revoke.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub federation_trust_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_manage_federation: bool,
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
        self.peer_transport.validate()?;
        let verifiers = self.validate_authorization_verifiers()?;
        if self.scopes.is_empty() || self.scopes.len() > 128 {
            return Err("execution authority requires 1..128 declared scopes".into());
        }
        let mut scopes = BTreeSet::new();
        for scope in &self.scopes {
            scope.validate_components()?;
            for profile in scope
                .agent_services
                .iter()
                .filter_map(|service| service.authorization.as_ref())
                .chain(
                    scope
                        .retained_agent_services
                        .iter()
                        .filter_map(|service| service.authorization.as_ref()),
                )
            {
                if !verifiers.contains(&(profile.verifier_id.as_str(), profile.verifier_revision)) {
                    return Err(
                        "agent service references an uninstalled authorization verifier".into(),
                    );
                }
            }
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

    fn validate_authorization_verifiers(&self) -> Result<BTreeSet<(&str, u64)>, String> {
        if self.authorization_verifiers.len() > 64 {
            return Err("too many task authorization verifiers".into());
        }
        let mut verifiers = BTreeSet::new();
        for verifier in &self.authorization_verifiers {
            verifier.validate()?;
            if !verifiers.insert((verifier.id.as_str(), verifier.revision)) {
                return Err("duplicate task authorization verifier revision".into());
            }
        }
        Ok(verifiers)
    }
}

impl ExecutionScopeConfig {
    fn validate_components(&self) -> Result<(), String> {
        self.validate_chains()?;
        self.validate_agents()?;
        self.validate_permits()?;
        self.validate_managers()?;
        self.validate_federation()?;
        self.validate_history_only()?;
        self.validate_reconciliation_only()
    }

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
                || !self.retained_agent_services.is_empty()
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
                || !self.retained_agent_services.is_empty()
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
        if self.agent_services.len() > 128
            || self.retained_agent_services.len() > 128
            || self.agent_services.len() + self.retained_agent_services.len() > 256
        {
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
        for source in &self.agent_services {
            let onward = source.onward_agents.iter().collect::<BTreeSet<_>>();
            if source.onward_agents.len() > 16 || onward.len() != source.onward_agents.len() {
                return Err("agent onward service bindings must be unique and bounded".into());
            }
            for target_id in onward {
                let target = self
                    .agent_services
                    .iter()
                    .find(|candidate| &candidate.card.agent_id == target_id)
                    .ok_or("agent onward service must name a current declared service")?;
                if target.card.agent_id == source.card.agent_id
                    || !target
                        .grants
                        .iter()
                        .any(|grant| grant.source == source.principal)
                    || !source.recipient_permits.iter().any(|reference| {
                        self.permits.iter().any(|permit| {
                            permit.id == reference.id
                                && permit.revision == reference.accepted_revision
                                && permit.subject == source.principal
                                && permit.agents.contains(target_id)
                        })
                    })
                {
                    return Err(
                        "agent onward service requires an explicit target grant and permit".into(),
                    );
                }
            }
        }
        let mut retained_epochs = BTreeSet::new();
        let mut retained_digests = BTreeSet::new();
        for service in &self.retained_agent_services {
            service.validate(self)?;
            if !retained_epochs.insert((&service.card.agent_id, service.registry_revision))
                || !retained_digests.insert(&service.binding_digest)
                || self.agent_services.iter().any(|current| {
                    current.card.agent_id == service.card.agent_id
                        && service.registry_revision >= current.registry_revision
                })
            {
                return Err("retained agent service bindings must be unique prior epochs".into());
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
                || manager.agents.len() > 128
                || manager.agents.iter().collect::<BTreeSet<_>>().len() != manager.agents.len()
                || (!manager.agents.is_empty() && !manager.can_intervene)
                || manager.agents.iter().any(|id| {
                    id.contains('*')
                        || ResourceRef::new(ResourceKind::Agent, &self.namespace, &self.tenant, id)
                            .is_err()
                })
                || manager.routes.len() > 128
                || manager.routes.iter().collect::<BTreeSet<_>>().len() != manager.routes.len()
                || manager.routes.iter().any(|r| !self.routes.contains(r))
                || (!manager.can_issue_permits
                    && !manager.can_intervene
                    && !manager.can_read_history
                    && !manager.can_reconcile
                    && !manager.can_manage_federation
                    && manager.workforce.is_none())
                || manager.federation_trust_ids.len() > 128
                || manager
                    .federation_trust_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != manager.federation_trust_ids.len()
                || manager.can_manage_federation == manager.federation_trust_ids.is_empty()
                || manager.federation_trust_ids.iter().any(|id| {
                    self.federation.as_ref().is_none_or(|federation| {
                        !federation.trusts.iter().any(|trust| trust.id == *id)
                    })
                })
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
            if manager.can_manage_federation {
                let federation = self
                    .federation
                    .as_ref()
                    .ok_or("federation management requires configured trusts")?;
                FederationTrustIssuanceCeiling {
                    issuer: manager.principal.clone(),
                    approved: federation
                        .trusts
                        .iter()
                        .filter(|trust| manager.federation_trust_ids.contains(&trust.id))
                        .cloned()
                        .collect(),
                    valid_from_ms: manager.valid_from_ms,
                    deadline_ms: manager.limits.deadline_ms,
                }
                .validate()
                .map_err(|_| "federation manager exceeds its trust publication ceiling")?;
            }
        }
        Ok(())
    }

    fn validate_federation(&self) -> Result<(), String> {
        let Some(federation) = &self.federation else {
            if self
                .managers
                .iter()
                .any(|manager| manager.can_manage_federation)
            {
                return Err("federation management requires a deployment federation block".into());
            }
            return Ok(());
        };
        if federation.local_audience.is_empty()
            || federation.local_audience.len() > 255
            || federation.local_audience.trim() != federation.local_audience
            || federation.local_audience.chars().any(char::is_control)
            || (federation.trusts.is_empty() && federation.exports.is_empty())
            || federation.trusts.is_empty() != federation.peers.is_empty()
            || federation.trusts.len() > 128
            || federation.peers.len() > 128
        {
            return Err("invalid bounded execution federation configuration".into());
        }
        let mut trust_revisions = BTreeSet::new();
        let mut trust_ids = BTreeSet::new();
        for trust in &federation.trusts {
            if trust.local_audience != federation.local_audience
                || !trust_revisions.insert((&trust.id, trust.revision))
                || trust.approved.iter().any(|shape| {
                    shape.agent_resource.namespace() != self.namespace
                        || shape.agent_resource.tenant() != self.tenant
                        || std::iter::once(&shape.ingress_effect)
                            .chain(&shape.effects)
                            .flat_map(|effect| &effect.resources)
                            .any(|resource| {
                                resource.namespace() != self.namespace
                                    || resource.tenant() != self.tenant
                            })
                })
            {
                return Err("federation trust exceeds its execution scope".into());
            }
            trust_ids.insert(&trust.id);
        }
        if !federation.trusts.is_empty() {
            FederationTrustIssuanceCeiling {
                issuer: self.publisher.clone(),
                approved: federation.trusts.clone(),
                valid_from_ms: self.valid_from_ms,
                deadline_ms: self.credential_limits.deadline_ms,
            }
            .validate()
            .map_err(|_| "invalid federation trust publication ceiling")?;
        }
        let mut peer_domains = BTreeSet::new();
        let mut peer_principals = BTreeSet::new();
        for peer in &federation.peers {
            let ids = peer.trust_ids.iter().collect::<BTreeSet<_>>();
            if peer.foreign_domain.is_empty()
                || peer.foreign_domain.len() > 255
                || peer.foreign_domain.trim() != peer.foreign_domain
                || peer.foreign_domain.chars().any(char::is_control)
                || !peer_domains.insert(&peer.foreign_domain)
                || !peer_principals.insert(peer.authentication_principal.id())
                || !self.subjects.contains(&peer.authentication_principal)
                || peer.trust_ids.is_empty()
                || peer.trust_ids.len() > 128
                || ids.len() != peer.trust_ids.len()
                || peer.trust_ids.iter().any(|id| !trust_ids.contains(id))
                || peer.trust_ids.iter().any(|id| {
                    federation
                        .trusts
                        .iter()
                        .filter(|trust| trust.id == *id)
                        .any(|trust| trust.foreign_domain != peer.foreign_domain)
                })
            {
                return Err("invalid authenticated federation peer binding".into());
            }
        }
        if federation.exports.len() > 128 {
            return Err("too many outbound federation profiles".into());
        }
        let mut export_ids = BTreeSet::new();
        for export in &federation.exports {
            if !export_ids.insert(&export.id)
                || export.grant.limits.deadline_ms > self.credential_limits.deadline_ms
                || !self.permits.iter().any(|permit| {
                    permit.subject == export.grant.source
                        && permit.federation_exports.contains(&export.id)
                })
            {
                return Err("outbound federation profile exceeds its execution scope".into());
            }
            export.validate(self, &federation.local_audience)?;
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
                    && permit.agents.is_empty()
                    && permit.federation_exports.is_empty())
                || permit.routes.len()
                    + permit.chains.len()
                    + permit.agents.len()
                    + permit.federation_exports.len()
                    > 128
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
                || permit
                    .federation_exports
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != permit.federation_exports.len()
                || permit.federation_exports.iter().any(|id| {
                    self.federation.as_ref().is_none_or(|federation| {
                        !federation
                            .exports
                            .iter()
                            .any(|export| export.id == *id && export.grant.source == permit.subject)
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

    fn federation_declaration() -> ExecutionAuthorityConfig {
        use acteon_governance::federation::{
            FederationEnvelopeSigner, FederationGrantCeiling, FederationTrust,
        };
        let mut configuration = declaration();
        let scope = &mut configuration.scopes[0];
        let source =
            PrincipalIdentity::new("agent/remote", acteon_core::PrincipalKind::Agent).unwrap();
        let target = scope.subjects[0].clone();
        let manager =
            PrincipalIdentity::new("operator/federation", acteon_core::PrincipalKind::Human)
                .unwrap();
        scope.subjects.extend([source.clone(), manager.clone()]);
        let agent_resource = ResourceRef::new(ResourceKind::Agent, "prod", "acme", "maya").unwrap();
        let endpoint =
            ResourceRef::new(ResourceKind::Endpoint, "prod", "acme", "incident").unwrap();
        let ingress = AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: vec![agent_resource.clone(), endpoint.clone()],
        };
        let trust = FederationTrust {
            id: "remote-prod".into(),
            revision: 1,
            foreign_domain: "remote.example".into(),
            local_audience: "local.example".into(),
            key_id: "remote-v1".into(),
            verifying_key: FederationEnvelopeSigner::from_secret([7; 32]).verifying_key_hex(),
            minimum_issuer_epoch: 1,
            max_revocation_staleness_ms: 30_000,
            max_clock_skew_ms: 1_000,
            valid_from_ms: 0,
            deadline_ms: scope.credential_limits.deadline_ms,
            approved: vec![FederationGrantCeiling {
                source: source.clone(),
                target,
                agent_resource,
                binding_digest: "a".repeat(64),
                skill: "notify".into(),
                ingress_effect: ingress,
                effects: vec![AcceptedEffect {
                    operation: "provider.execute".into(),
                    resources: vec![endpoint],
                }],
                limits: RootBudgetLimits {
                    max_units: 5,
                    max_concurrent: 1,
                    deadline_ms: scope.credential_limits.deadline_ms,
                },
                max_depth: 2,
            }],
        };
        scope.federation = Some(ExecutionFederationConfig {
            local_audience: "local.example".into(),
            trusts: vec![trust],
            peers: vec![FederationPeerConfig {
                foreign_domain: "remote.example".into(),
                authentication_principal: source.clone(),
                trust_ids: vec!["remote-prod".into()],
            }],
            exports: vec![],
        });
        scope.managers.push(ExecutionManagerConfig {
            agents: vec![],
            principal: manager,
            subjects: vec![source],
            routes: vec![],
            can_issue_permits: false,
            can_intervene: false,
            can_read_history: false,
            can_reconcile: false,
            federation_trust_ids: vec!["remote-prod".into()],
            can_manage_federation: true,
            reconciliation_resources: vec![],
            workforce: None,
            valid_from_ms: 0,
            limits: scope.credential_limits.clone(),
        });
        configuration
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

    #[test]
    fn federation_requires_exact_audience_peer_scope_and_independent_manager_rights() {
        let configuration = federation_declaration();
        configuration
            .validate(("auth-control", "deployment"))
            .unwrap();
        for variation in 0..7 {
            let mut invalid = configuration.clone();
            match variation {
                0 => {
                    invalid.scopes[0].federation.as_mut().unwrap().trusts[0].local_audience =
                        "another.example".into()
                }
                1 => {
                    invalid.scopes[0].federation.as_mut().unwrap().peers[0].foreign_domain =
                        "impostor.example".into()
                }
                2 => {
                    invalid.scopes[0].federation.as_mut().unwrap().peers[0].trust_ids =
                        vec!["unknown".into()]
                }
                3 => {
                    invalid.scopes[0].federation.as_mut().unwrap().trusts[0].approved[0]
                        .agent_resource =
                        ResourceRef::new(ResourceKind::Agent, "prod", "other", "maya").unwrap()
                }
                4 => invalid.scopes[0].managers[0].can_manage_federation = false,
                5 => invalid.scopes[0].managers[0].federation_trust_ids.clear(),
                _ => {
                    let duplicate = invalid.scopes[0].federation.as_ref().unwrap().peers[0].clone();
                    invalid.scopes[0]
                        .federation
                        .as_mut()
                        .unwrap()
                        .peers
                        .push(duplicate);
                }
            }
            assert!(invalid.validate(("auth-control", "deployment")).is_err());
        }
    }
}
