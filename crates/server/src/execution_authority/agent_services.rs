//! Operator-qualified individual agent services. Wire data cannot install one.
use acteon_core::{AgentCard, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_executor::{
    delegation::{ApprovedPeerBinding, ApprovedServicePlan},
    governed::BoundProvider,
};
use acteon_governance::{RootBudgetLimits, context::AcceptedEffect, permit::PermitReference};
use serde::{Deserialize, Serialize};
use sha2::Digest;

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
    /// Fixed trust policy for authorization challenges opened by this service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<AgentServiceAuthorizationProfile>,
    /// Exact agent services this runtime may invoke from accepted work. The
    /// host resolves these IDs to reviewed bindings; models cannot supply URLs,
    /// credentials, permits, or delegation grants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub onward_agents: Vec<String>,
    /// Replay qualification of this service's exact message endpoint.
    #[serde(default)]
    pub submission_capability: acteon_executor::delegation::PeerSubmissionCapability,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<AgentServiceAuthorizationProfile>,
    /// Operator-reviewed digest of the complete historical service binding.
    pub binding_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentServiceAuthorizationProfile {
    pub verifier_id: String,
    pub verifier_revision: u64,
    pub credential_authority: String,
    pub audience: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_scopes: Vec<String>,
    #[serde(default = "default_authorization_ttl_ms")]
    pub challenge_ttl_ms: u64,
}

fn default_authorization_ttl_ms() -> u64 {
    300_000
}

impl AgentServiceAuthorizationProfile {
    fn validate(&self) -> bool {
        let scopes = self
            .required_scopes
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        valid_identifier(&self.verifier_id)
            && self.verifier_revision > 0
            && valid_text(&self.credential_authority)
            && valid_text(&self.audience)
            && self.required_scopes.len() <= 32
            && scopes.len() == self.required_scopes.len()
            && self.required_scopes.iter().all(|scope| valid_text(scope))
            && (1_000..=86_400_000).contains(&self.challenge_ttl_ms)
    }
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
            || self
                .authorization
                .as_ref()
                .is_some_and(|profile| !profile.validate())
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

    pub(crate) fn qualify_with_plan(
        &self,
        bound: &BoundProvider,
        intent: Vec<AcceptedEffect>,
        direct: Vec<AcceptedEffect>,
    ) -> Result<ApprovedPeerBinding, String> {
        qualify_binding(
            ServiceBindingDeclaration {
                card: &self.card,
                registry_revision: self.registry_revision,
                principal: &self.principal,
                skill: &self.skill,
                endpoint: &self.endpoint,
                endpoint_id: &self.endpoint_id,
                route: &self.route,
                authorization: self.authorization.as_ref(),
            },
            bound,
            intent,
            direct,
        )
    }

    pub(crate) fn ingress_effect(
        &self,
        resources: Vec<ResourceRef>,
    ) -> Result<AcceptedEffect, String> {
        service_ingress(
            &self.card,
            self.registry_revision,
            &self.endpoint_id,
            resources,
            self.authorization.as_ref(),
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
            || self
                .authorization
                .as_ref()
                .is_some_and(|profile| !profile.validate())
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
                authorization: self.authorization.as_ref(),
            },
            bound,
            vec![bound.effect().clone()],
            vec![bound.effect().clone()],
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
    authorization: Option<&'a AgentServiceAuthorizationProfile>,
}

fn qualify_binding(
    declaration: ServiceBindingDeclaration<'_>,
    bound: &BoundProvider,
    intent: Vec<AcceptedEffect>,
    direct: Vec<AcceptedEffect>,
) -> Result<ApprovedPeerBinding, String> {
    if bound.provider_name() != declaration.route.provider
        || bound.action_type() != declaration.route.action_type
        || bound.catalog_version().is_none()
    {
        return Err("agent service has no qualified actual operation".into());
    }
    let resources: Vec<_> = intent
        .iter()
        .flat_map(|effect| effect.resources.iter().cloned())
        .collect();
    let ingress = service_ingress(
        declaration.card,
        declaration.registry_revision,
        declaration.endpoint_id,
        resources,
        declaration.authorization,
    )?;
    ApprovedPeerBinding::new_service_trusted(
        declaration.card,
        declaration.principal.clone(),
        declaration.skill,
        declaration.endpoint,
        "rest",
        ingress,
        ApprovedServicePlan::new_trusted(intent, direct).map_err(|_| "invalid service plan")?,
    )
    .map_err(|_| "invalid approved service binding".into())
}

fn service_ingress(
    card: &AgentCard,
    registry_revision: u64,
    endpoint_id: &str,
    mut resources: Vec<ResourceRef>,
    authorization: Option<&AgentServiceAuthorizationProfile>,
) -> Result<AcceptedEffect, String> {
    // The card revision is an enclosing resource, separate from the digest
    // of the complete service binding (which also includes this footprint).
    let digest =
        acteon_executor::delegation::card_digest(card).map_err(|_| "invalid service card")?;
    for (kind, name) in [
        (ResourceKind::Agent, card.agent_id.clone()),
        (
            ResourceKind::Route,
            format!("agent-registry.{}.{}", card.agent_id, registry_revision),
        ),
        (ResourceKind::Endpoint, endpoint_id.to_owned()),
        (ResourceKind::Route, format!("agent-card.{digest}")),
    ] {
        resources.push(
            ResourceRef::new(kind, &card.namespace, &card.tenant, name)
                .map_err(|_| "invalid service enclosing resource")?,
        );
    }
    if let Some(profile) = authorization {
        let digest = format!(
            "{:x}",
            sha2::Sha256::digest(
                serde_json::to_vec(profile).map_err(|_| "invalid authorization profile")?
            )
        );
        resources.push(
            ResourceRef::new(
                ResourceKind::Route,
                &card.namespace,
                &card.tenant,
                format!("agent-authorization.{digest}"),
            )
            .map_err(|_| "invalid authorization profile resource")?,
        );
    }
    resources.sort();
    resources.dedup();
    Ok(AcceptedEffect {
        operation: "agent.invoke".into(),
        resources,
    })
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
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
    pub authorization: Option<AgentServiceAuthorizationProfile>,
    pub binding: ApprovedPeerBinding,
    pub bound: BoundProvider,
}
