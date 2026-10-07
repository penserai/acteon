//! Operator-qualified individual agent services. Wire data cannot install one.
use acteon_core::{AgentCard, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_executor::{
    delegation::{ApprovedPeerBinding, ApprovedServicePlan},
    governed::BoundProvider,
};
use acteon_governance::{RootBudgetLimits, context::AcceptedEffect, permit::PermitReference};
use serde::{Deserialize, Serialize};

use crate::config::{ExecutionRouteConfig, ExecutionScopeConfig};

/// Complete reviewed service declaration, including independent service credentials.
/// Secrets are resolved by the host and are never accepted in invocation bodies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentServiceDeclaration {
    pub card: AgentCard,
    /// Monotonic reviewed registry epoch; a retired epoch cannot be republished.
    #[serde(default = "initial_registry_revision")]
    pub registry_revision: u64,
    pub principal: PrincipalIdentity,
    pub skill: String,
    pub endpoint: String,
    pub endpoint_id: String,
    pub route: ExecutionRouteConfig,
    pub recipient_key_env: String,
    pub recipient_permits: Vec<PermitReference>,
    pub grants: Vec<AgentServiceGrantDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentServiceGrantDeclaration {
    pub id: String,
    pub revision: u64,
    pub source: PrincipalIdentity,
    pub source_permits: Vec<PermitReference>,
    pub valid_from_ms: i64,
    pub limits: RootBudgetLimits,
    pub max_depth: usize,
}

/// A previously qualified service binding retained only for accepted-work
/// observation and recovery. It cannot publish registry authority, grants, or
/// permits, and it cannot accept new messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedAgentServiceDeclaration {
    pub card: AgentCard,
    pub registry_revision: u64,
    pub principal: PrincipalIdentity,
    pub skill: String,
    pub endpoint: String,
    pub endpoint_id: String,
    pub route: ExecutionRouteConfig,
    /// Operator-reviewed digest of the complete historical service binding.
    pub binding_digest: String,
}

impl AgentServiceDeclaration {
    pub(crate) fn validate(&self, scope: &ExecutionScopeConfig) -> Result<(), String> {
        self.card.validate().map_err(|_| "invalid service card")?;
        ResourceRef::new(
            ResourceKind::Endpoint,
            &scope.namespace,
            &scope.tenant,
            &self.endpoint_id,
        )
        .map_err(|_| "invalid service endpoint identity")?;
        let env = self.recipient_key_env.as_bytes();
        if self.registry_revision == 0
            || self.card.namespace != scope.namespace
            || self.card.tenant != scope.tenant
            || self.principal.kind() != PrincipalKind::Agent
            || !scope.subjects.contains(&self.principal)
            || !scope.routes.contains(&self.route)
            || env.is_empty()
            || env.len() > 128
            || !env[0].is_ascii_alphabetic()
            || !env.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
            || !valid_permits(&self.recipient_permits)
            || self.grants.is_empty()
            || self.grants.len() > 16
        {
            return Err("invalid independently declared agent service".into());
        }
        let mut sources = std::collections::BTreeSet::new();
        let mut grants = std::collections::BTreeSet::new();
        for grant in &self.grants {
            ResourceRef::new(
                ResourceKind::Agent,
                &scope.namespace,
                &scope.tenant,
                &grant.id,
            )
            .map_err(|_| "invalid service grant identity")?;
            if grant.source == self.principal
                || grant.revision == 0
                || !sources.insert(grant.source.id())
                || !grants.insert(&grant.id)
                || !scope.subjects.contains(&grant.source)
                || !valid_permits(&grant.source_permits)
                || grant.valid_from_ms < scope.valid_from_ms
                || grant.limits.deadline_ms <= grant.valid_from_ms
                || grant.limits.deadline_ms > scope.credential_limits.deadline_ms
                || grant.limits.max_units == 0
                || grant.limits.max_units > scope.credential_limits.max_units
                || grant.limits.max_concurrent == 0
                || grant.limits.max_concurrent > scope.credential_limits.max_concurrent
                || grant.max_depth == 0
                || grant.max_depth >= 16
            {
                return Err("service grant exceeds independent publication bounds".into());
            }
        }
        Ok(())
    }

    pub(crate) fn qualify(&self, bound: &BoundProvider) -> Result<ApprovedPeerBinding, String> {
        qualify_binding(
            ServiceBindingDeclaration {
                card: &self.card,
                registry_revision: self.registry_revision,
                principal: &self.principal,
                skill: &self.skill,
                endpoint: &self.endpoint,
                endpoint_id: &self.endpoint_id,
                route: &self.route,
            },
            bound,
        )
    }
}

impl RetainedAgentServiceDeclaration {
    pub(crate) fn validate(&self, scope: &ExecutionScopeConfig) -> Result<(), String> {
        self.card
            .validate()
            .map_err(|_| "invalid retained service card")?;
        ResourceRef::new(
            ResourceKind::Endpoint,
            &scope.namespace,
            &scope.tenant,
            &self.endpoint_id,
        )
        .map_err(|_| "invalid retained service endpoint identity")?;
        if self.registry_revision == 0
            || self.card.namespace != scope.namespace
            || self.card.tenant != scope.tenant
            || self.principal.kind() != PrincipalKind::Agent
            || !scope.subjects.contains(&self.principal)
            || !scope.routes.contains(&self.route)
            || !valid_digest(&self.binding_digest)
        {
            return Err("invalid retained agent service".into());
        }
        Ok(())
    }

    pub(crate) fn qualify(&self, bound: &BoundProvider) -> Result<ApprovedPeerBinding, String> {
        let binding = qualify_binding(
            ServiceBindingDeclaration {
                card: &self.card,
                registry_revision: self.registry_revision,
                principal: &self.principal,
                skill: &self.skill,
                endpoint: &self.endpoint,
                endpoint_id: &self.endpoint_id,
                route: &self.route,
            },
            bound,
        )?;
        if binding.digest() != self.binding_digest {
            return Err("retained agent service digest mismatch".into());
        }
        Ok(binding)
    }
}

#[derive(Clone, Copy)]
struct ServiceBindingDeclaration<'a> {
    card: &'a AgentCard,
    registry_revision: u64,
    principal: &'a PrincipalIdentity,
    skill: &'a str,
    endpoint: &'a str,
    endpoint_id: &'a str,
    route: &'a ExecutionRouteConfig,
}

fn qualify_binding(
    declaration: ServiceBindingDeclaration<'_>,
    bound: &BoundProvider,
) -> Result<ApprovedPeerBinding, String> {
    if bound.provider_name() != declaration.route.provider
        || bound.action_type() != declaration.route.action_type
        || bound.catalog_version().is_none()
    {
        return Err("agent service has no qualified actual operation".into());
    }
    // The card revision is an enclosing resource, separate from the digest
    // of the complete service binding (which also includes this footprint).
    let digest = acteon_executor::delegation::card_digest(declaration.card)
        .map_err(|_| "invalid service card")?;
    let mut resources = bound.effect().resources.clone();
    for (kind, name) in [
        (ResourceKind::Agent, declaration.card.agent_id.clone()),
        (
            ResourceKind::Route,
            format!(
                "agent-registry.{}.{}",
                declaration.card.agent_id, declaration.registry_revision
            ),
        ),
        (ResourceKind::Endpoint, declaration.endpoint_id.to_owned()),
        (ResourceKind::Route, format!("agent-card.{digest}")),
    ] {
        resources.push(
            ResourceRef::new(
                kind,
                &declaration.card.namespace,
                &declaration.card.tenant,
                name,
            )
            .map_err(|_| "invalid service enclosing resource")?,
        );
    }
    resources.sort();
    let ingress = AcceptedEffect {
        operation: "agent.invoke".into(),
        resources,
    };
    ApprovedPeerBinding::new_service_trusted(
        declaration.card,
        declaration.principal.clone(),
        declaration.skill,
        declaration.endpoint,
        "rest",
        ingress,
        ApprovedServicePlan::new_trusted(
            vec![bound.effect().clone()],
            vec![bound.effect().clone()],
        )
        .map_err(|_| "invalid service plan")?,
    )
    .map_err(|_| "invalid approved service binding".into())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn initial_registry_revision() -> u64 {
    1
}

fn valid_permits(permits: &[PermitReference]) -> bool {
    !permits.is_empty()
        && permits.len() <= 16
        && permits
            .iter()
            .all(|p| !p.id.is_empty() && p.id.len() <= 120 && p.accepted_revision > 0)
        && permits
            .iter()
            .map(|p| &p.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == permits.len()
}

#[derive(Clone)]
pub(crate) struct PreparedAgentService {
    pub declaration: AgentServiceDeclaration,
    pub binding: ApprovedPeerBinding,
    pub bound: BoundProvider,
}

#[derive(Clone)]
pub(crate) struct PreparedRetainedAgentService {
    pub agent_id: String,
    pub binding: ApprovedPeerBinding,
    pub bound: BoundProvider,
}
