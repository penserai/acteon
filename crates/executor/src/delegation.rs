//! Approved individual registry bindings and advisory peer discovery.
//!
//! This reads the configured `StateStore`'s existing agent/card records. Cards
//! advertise capabilities; only trusted host bindings qualify a destination.
//! Neither a discovered candidate nor a registry URL authorizes an A2A send.
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::{
    Agent, AgentCard, AgentStatus, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef,
};
use acteon_governance::{
    AuthorityCoordinator, AuthorityStamp, CoordinationError,
    context::{AcceptedEffect, VerifiedExecutionContext},
    delegation::{
        DelegationDiscoveryRequest, ServiceDelegationDiscoveryRequest, ServiceDiscoveryBinding,
    },
    delegation_policy::DelegationGrantReference,
    permit::PermitReference,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_time::Clock;
use sha2::{Digest, Sha256};

const MAX_BINDINGS: usize = 128;
/// Maximum serialized UTF-8 bytes in an approved agent/card registry record.
/// Hosts publishing discoverable metadata must enforce this bound before writes.
pub const MAX_PEER_REGISTRY_RECORD_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PeerDiscoveryError {
    #[error("invalid or unqualified peer registry binding")]
    Binding,
    #[error("peer registry capacity exceeded")]
    Capacity,
    #[error("peer registry unavailable or corrupt")]
    Unavailable,
    #[error(transparent)]
    Authority(#[from] CoordinationError),
}

/// A host-approved immutable individual card/skill/endpoint/actor/effect tuple.
/// No `Deserialize`: model fields or a published card cannot install approval.
/// Credential exchange and actual network confinement belong to the subsequent
/// transport adapter; this object alone grants neither execution nor networking.
#[derive(Clone)]
pub struct ApprovedPeerBinding {
    namespace: String,
    tenant: String,
    agent_id: String,
    target: PrincipalIdentity,
    skill: String,
    card_digest: String,
    endpoint: String,
    transport: String,
    digest: String,
    effect: AcceptedEffect,
    agent_resource: ResourceRef,
    service: Option<ApprovedServicePlan>,
}
impl ApprovedPeerBinding {
    /// Approval is an independent operator decision. HTTPS syntax validation is
    /// not an SSRF policy, endpoint authentication or protocol qualification.
    pub fn new_trusted(
        card: &AgentCard,
        target: PrincipalIdentity,
        skill: &str,
        endpoint: &str,
        transport: &str,
        effect: AcceptedEffect,
    ) -> Result<Self, PeerDiscoveryError> {
        card.validate().map_err(|_| PeerDiscoveryError::Binding)?;
        let url = url::Url::parse(endpoint).map_err(|_| PeerDiscoveryError::Binding)?;
        let resource = ResourceRef::new(
            ResourceKind::Agent,
            &card.namespace,
            &card.tenant,
            &card.agent_id,
        )
        .map_err(|_| PeerDiscoveryError::Binding)?;
        if target.kind() != PrincipalKind::Agent
            || !matches!(transport, "rest" | "json-rpc")
            || endpoint.len() > 2048
            || url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !card
                .interfaces
                .iter()
                .any(|i| i.kind == transport && i.url == endpoint)
            || card.skills.iter().filter(|s| s.name == skill).count() != 1
            || effect.operation != "agent.invoke"
            || effect.resources.is_empty()
            || effect.resources.len() > 16
            || !effect.resources.contains(&resource)
            || effect
                .resources
                .iter()
                .any(|r| r.namespace() != card.namespace || r.tenant() != card.tenant)
            || effect
                .resources
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != effect.resources.len()
        {
            return Err(PeerDiscoveryError::Binding);
        }
        let card_digest = card_digest(card)?;
        let digest = value_digest(&serde_json::json!({
            "domain":"acteon.approved-peer-discovery.v1", "card_digest":card_digest,
            "target":target, "skill":skill, "endpoint":endpoint, "transport":transport,
            "effect":effect,
        }))?;
        Ok(Self {
            namespace: card.namespace.clone(),
            tenant: card.tenant.clone(),
            agent_id: card.agent_id.clone(),
            target,
            skill: skill.into(),
            card_digest,
            endpoint: endpoint.into(),
            transport: transport.into(),
            digest,
            effect,
            agent_resource: resource,
            service: None,
        })
    }
    /// Pin the complete downstream intent and recipient direct operations into
    /// approval. A changed plan requires a different binding and accepted grant.
    pub fn new_service_trusted(
        card: &AgentCard,
        target: PrincipalIdentity,
        skill: &str,
        endpoint: &str,
        transport: &str,
        ingress: AcceptedEffect,
        plan: ApprovedServicePlan,
    ) -> Result<Self, PeerDiscoveryError> {
        let mut binding = Self::new_trusted(card, target, skill, endpoint, transport, ingress)?;
        if plan
            .intent
            .iter()
            .flat_map(|e| &e.resources)
            .any(|r| !binding.effect.resources.contains(r))
        {
            return Err(PeerDiscoveryError::Binding);
        }
        binding.digest = value_digest(&serde_json::json!({
            "domain": "acteon.approved-service-discovery.v1",
            "binding": binding.digest,
            "intent": plan.intent,
            "direct_effects": plan.direct_effects,
        }))?;
        binding.service = Some(plan);
        Ok(binding)
    }
    /// Complete approved invocation footprint; observation supplies no authority.
    #[must_use]
    pub fn ingress_effect(&self) -> &AcceptedEffect {
        &self.effect
    }

    #[must_use]
    pub fn target(&self) -> &PrincipalIdentity {
        &self.target
    }
    #[must_use]
    pub fn agent_resource(&self) -> &ResourceRef {
        &self.agent_resource
    }
    #[must_use]
    pub fn service_plan(&self) -> Option<&ApprovedServicePlan> {
        self.service.as_ref()
    }
    fn service_binding(&self) -> Option<ServiceDiscoveryBinding<'_>> {
        self.service.as_ref().map(|plan| ServiceDiscoveryBinding {
            target: &self.target,
            agent_resource: &self.agent_resource,
            binding_digest: &self.digest,
            skill: &self.skill,
            ingress: &self.effect,
            intent: &plan.intent,
            direct_effects: &plan.direct_effects,
        })
    }
    async fn check_source(
        &self,
        coordinator: &AuthorityCoordinator,
        parent: &VerifiedExecutionContext,
        permits: &[PermitReference],
        clock: &dyn Clock,
    ) -> Result<(), CoordinationError> {
        if let Some(binding) = self.service_binding() {
            coordinator
                .check_service_delegation_source(parent, permits, binding, clock)
                .await?;
            Ok(())
        } else {
            coordinator
                .check_delegation_source(parent, permits, &self.effect, clock)
                .await
        }
    }
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
    /// Host transport adapters must separately enforce network and protocol policy.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    #[must_use]
    pub fn transport(&self) -> &str {
        &self.transport
    }
}
/// Operator-approved service footprint. No deserialization or mutable fields:
/// agent advertisements cannot extend this intent after approval.
#[derive(Clone)]
pub struct ApprovedServicePlan {
    intent: Vec<AcceptedEffect>,
    direct_effects: Vec<AcceptedEffect>,
}
impl ApprovedServicePlan {
    #[must_use]
    pub fn intent(&self) -> &[AcceptedEffect] {
        &self.intent
    }
    #[must_use]
    pub fn direct_effects(&self) -> &[AcceptedEffect] {
        &self.direct_effects
    }
    pub fn new_trusted(
        intent: Vec<AcceptedEffect>,
        direct_effects: Vec<AcceptedEffect>,
    ) -> Result<Self, PeerDiscoveryError> {
        let valid = |effects: &[AcceptedEffect]| {
            !effects.is_empty()
                && effects.len() <= 128
                && effects.iter().enumerate().all(|(i, e)| {
                    !e.operation.is_empty()
                        && e.operation.len() <= 1024
                        && e.operation != "*"
                        && !e.operation.chars().any(char::is_control)
                        && !e.resources.is_empty()
                        && e.resources.len() <= 16
                        && e.resources
                            .iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            == e.resources.len()
                        && !effects[..i]
                            .iter()
                            .any(|a| acteon_governance::permit::matches_effect(a, e))
                })
        };
        if !valid(&intent)
            || !valid(&direct_effects)
            || direct_effects.iter().any(|e| {
                !intent
                    .iter()
                    .any(|a| acteon_governance::permit::matches_effect(a, e))
            })
        {
            return Err(PeerDiscoveryError::Binding);
        }
        Ok(Self {
            intent,
            direct_effects,
        })
    }
}
fn value_digest(value: &serde_json::Value) -> Result<String, PeerDiscoveryError> {
    let bytes = crate::plan::canonical_bytes(value).map_err(|_| PeerDiscoveryError::Binding)?;
    if bytes.len() > MAX_PEER_REGISTRY_RECORD_BYTES {
        return Err(PeerDiscoveryError::Capacity);
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
/// Deterministic complete-card digest; hashing supplies no execution approval.
pub fn card_digest(card: &AgentCard) -> Result<String, PeerDiscoveryError> {
    value_digest(&serde_json::to_value(card).map_err(|_| PeerDiscoveryError::Binding)?)
}

fn ordinary_denial(error: &CoordinationError) -> bool {
    matches!(
        error,
        CoordinationError::Restricted
            | CoordinationError::PermitDenied(_)
            | CoordinationError::BudgetExhausted
            | CoordinationError::ConcurrencyExhausted
            | CoordinationError::DeadlineExceeded
    )
}

/// Public descriptive fields are untrusted data, including prompt injection.
/// Deliberately excludes endpoint URLs, credentials and runtime context handles.
#[derive(Debug, Clone)]
pub struct PeerCandidate {
    pub agent_id: String,
    pub target: PrincipalIdentity,
    pub skill: String,
    pub description: Option<String>,
    pub card_version: String,
    pub binding_digest: String,
    pub accepted_grant: Option<DelegationGrantReference>,
    pub observed_authority: AuthorityStamp,
    pub checked_at_ms: i64,
}

/// Trusted independently authenticated ceilings supplied by the host. The
/// recipient's current permission is checked separately from the caller's.
pub struct PeerDiscoveryAuthority<'a> {
    pub coordinator: &'a AuthorityCoordinator,
    pub parent: &'a VerifiedExecutionContext,
    pub parent_permits: &'a [PermitReference],
    pub recipient: &'a VerifiedExecutionContext,
    pub recipient_permits: &'a [PermitReference],
    pub clock: &'a dyn Clock,
}

/// Independently authenticated recipient ceilings. They do not authorize a
/// delegation, and discovery never transfers their execution context to callers.
pub struct RecipientDiscoveryContext {
    pub context: VerifiedExecutionContext,
    pub permits: Vec<PermitReference>,
}

/// Trusted local host/runtime resolution, not model-supplied identity data.
/// Resolve retained authentication/context records without invoking or probing
/// peers. External credential exchange and probes need their own effect permits.
#[async_trait::async_trait]
pub trait PeerRecipientResolver: Send + Sync {
    async fn resolve(
        &self,
        namespace: &str,
        tenant: &str,
        target: &PrincipalIdentity,
    ) -> Result<Option<RecipientDiscoveryContext>, PeerDiscoveryError>;
}

pub struct PeerCandidateQuery<'a> {
    pub coordinator: &'a AuthorityCoordinator,
    pub parent: &'a VerifiedExecutionContext,
    pub parent_permits: &'a [PermitReference],
    pub recipients: &'a dyn PeerRecipientResolver,
    pub clock: &'a dyn Clock,
}

pub struct ApprovedPeerRegistry {
    state: Arc<dyn StateStore>,
    namespace: String,
    tenant: String,
    bindings: BTreeMap<(String, String), ApprovedPeerBinding>,
}
impl ApprovedPeerRegistry {
    pub fn new_trusted(
        state: Arc<dyn StateStore>,
        namespace: &str,
        tenant: &str,
        bindings: Vec<ApprovedPeerBinding>,
    ) -> Result<Self, PeerDiscoveryError> {
        if bindings.is_empty() || bindings.len() > MAX_BINDINGS {
            return Err(PeerDiscoveryError::Capacity);
        }
        let mut entries = BTreeMap::new();
        for binding in bindings {
            if binding.namespace != namespace
                || binding.tenant != tenant
                || entries
                    .insert((binding.agent_id.clone(), binding.skill.clone()), binding)
                    .is_some()
            {
                return Err(PeerDiscoveryError::Binding);
            }
        }
        Ok(Self {
            state,
            namespace: namespace.into(),
            tenant: tenant.into(),
            bindings: entries,
        })
    }
    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        kind: KeyKind,
        id: &str,
    ) -> Result<Option<T>, PeerDiscoveryError> {
        let raw = self
            .state
            .get(&StateKey::new(
                self.namespace.as_str(),
                self.tenant.as_str(),
                kind,
                id,
            ))
            .await
            .map_err(|_| PeerDiscoveryError::Unavailable)?;
        raw.map(|raw| {
            if raw.len() > MAX_PEER_REGISTRY_RECORD_BYTES {
                return Err(PeerDiscoveryError::Unavailable);
            }
            serde_json::from_str(&raw).map_err(|_| PeerDiscoveryError::Unavailable)
        })
        .transpose()
    }
    async fn inspect_binding(
        &self,
        binding: &ApprovedPeerBinding,
        clock: &dyn Clock,
    ) -> Result<Option<AgentCard>, PeerDiscoveryError> {
        let Some(agent): Option<Agent> = self.read(KeyKind::BusAgent, &binding.agent_id).await?
        else {
            return Ok(None);
        };
        let Some(card): Option<AgentCard> =
            self.read(KeyKind::BusAgentCard, &binding.agent_id).await?
        else {
            return Ok(None);
        };
        let now = clock.now();
        if agent.validate().is_err()
            || card.validate().is_err()
            || agent.agent_id != binding.agent_id
            || agent.namespace != self.namespace
            || agent.tenant != self.tenant
        {
            return Err(PeerDiscoveryError::Unavailable);
        }
        if !agent.effective_admin_state_at(now).is_routable()
            || agent.status_at(now) != AgentStatus::Online
            || card_digest(&card)? != binding.card_digest
        {
            return Ok(None);
        }
        Ok(Some(card))
    }
    /// Enumerate the bounded approved registry and filter individual candidates
    /// through both participants' current ceilings. No partial response survives
    /// a backend, corruption or stale-authority error; ordinary denied candidates
    /// are omitted. Descriptions are data for agent selection, never instructions.
    pub async fn discover_candidates(
        &self,
        skill: &str,
        query: PeerCandidateQuery<'_>,
    ) -> Result<Vec<PeerCandidate>, PeerDiscoveryError> {
        if skill.is_empty() || skill.len() > 120 {
            return Err(PeerDiscoveryError::Binding);
        }
        let mut candidates = Vec::new();
        for binding in self.bindings.values().filter(|b| b.skill == skill) {
            match binding
                .check_source(
                    query.coordinator,
                    query.parent,
                    query.parent_permits,
                    query.clock,
                )
                .await
            {
                Ok(()) => {}
                Err(error) if ordinary_denial(&error) => continue,
                Err(error) => return Err(PeerDiscoveryError::Authority(error)),
            }
            let Some(recipient) = query
                .recipients
                .resolve(&self.namespace, &self.tenant, &binding.target)
                .await?
            else {
                continue;
            };
            match self
                .discover_peer(
                    &binding.agent_id,
                    skill,
                    PeerDiscoveryAuthority {
                        coordinator: query.coordinator,
                        parent: query.parent,
                        parent_permits: query.parent_permits,
                        recipient: &recipient.context,
                        recipient_permits: &recipient.permits,
                        clock: query.clock,
                    },
                )
                .await
            {
                Ok(Some(candidate)) => candidates.push(candidate),
                Ok(None)
                | Err(PeerDiscoveryError::Authority(
                    CoordinationError::Restricted
                    | CoordinationError::PermitDenied(_)
                    | CoordinationError::BudgetExhausted
                    | CoordinationError::ConcurrencyExhausted
                    | CoordinationError::DeadlineExceeded,
                )) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(candidates)
    }

    /// Resolve only an operator-approved individual peer; model selection cannot
    /// supply an endpoint or approval. Re-read registry and authority for every
    /// preview. Reads across records are not an atomic invocation fence.
    pub async fn discover_peer(
        &self,
        agent_id: &str,
        skill: &str,
        authority: PeerDiscoveryAuthority<'_>,
    ) -> Result<Option<PeerCandidate>, PeerDiscoveryError> {
        let Some(binding) = self.bindings.get(&(agent_id.into(), skill.into())) else {
            return Ok(None);
        };
        binding
            .check_source(
                authority.coordinator,
                authority.parent,
                authority.parent_permits,
                authority.clock,
            )
            .await?;
        let Some(card) = self.inspect_binding(binding, authority.clock).await? else {
            return Ok(None);
        };
        let eligibility = if let Some(service) = binding.service_binding() {
            authority
                .coordinator
                .discover_service_delegation_eligibility(ServiceDelegationDiscoveryRequest {
                    parent: authority.parent,
                    parent_permits: authority.parent_permits,
                    recipient: authority.recipient,
                    recipient_permits: authority.recipient_permits,
                    binding: service,
                    clock: authority.clock,
                })
                .await?
        } else {
            authority
                .coordinator
                .discover_delegation_eligibility(DelegationDiscoveryRequest {
                    parent: authority.parent,
                    parent_permits: authority.parent_permits,
                    recipient: authority.recipient,
                    recipient_permits: authority.recipient_permits,
                    target: &binding.target,
                    agent_resource: &binding.agent_resource,
                    effect: &binding.effect,
                    clock: authority.clock,
                })
                .await?
        };
        Ok(Some(PeerCandidate {
            agent_id: binding.agent_id.clone(),
            target: eligibility.target().clone(),
            skill: binding.skill.clone(),
            description: card
                .skills
                .iter()
                .find(|s| s.name == binding.skill)
                .and_then(|s| s.description.clone()),
            card_version: card.version,
            binding_digest: binding.digest.clone(),
            accepted_grant: eligibility.grant().cloned(),
            observed_authority: eligibility.authority().clone(),
            checked_at_ms: eligibility.checked_at_ms(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::bus_agent_card::Interface;
    use acteon_core::{AgentAdminState, Skill};
    use acteon_state_memory::MemoryStateStore;
    use acteon_time::ManualClock;

    fn target() -> PrincipalIdentity {
        PrincipalIdentity::new("responder-actor", PrincipalKind::Agent).unwrap()
    }
    fn card() -> AgentCard {
        let mut card = AgentCard::new("responder", "city", "tenant", "Incident responder", "v1");
        card.skills.push(Skill::new("diagnose"));
        card.interfaces.push(Interface {
            kind: "rest".into(),
            url: "https://peer.example/a2a".into(),
        });
        card
    }
    fn effect() -> AcceptedEffect {
        AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: vec![
                ResourceRef::new(ResourceKind::Agent, "city", "tenant", "responder").unwrap(),
            ],
        }
    }
    fn binding(card: &AgentCard) -> ApprovedPeerBinding {
        ApprovedPeerBinding::new_trusted(
            card,
            target(),
            "diagnose",
            "https://peer.example/a2a",
            "rest",
            effect(),
        )
        .unwrap()
    }
    fn clock() -> ManualClock {
        ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap())
    }
    async fn publish(store: &dyn StateStore, agent: &Agent, card: &AgentCard) {
        for (kind, raw) in [
            (KeyKind::BusAgent, serde_json::to_string(agent).unwrap()),
            (KeyKind::BusAgentCard, serde_json::to_string(card).unwrap()),
        ] {
            store
                .set(
                    &StateKey::new("city", "tenant", kind, "responder"),
                    &raw,
                    None,
                )
                .await
                .unwrap();
        }
    }
    fn agent() -> Agent {
        let mut agent = Agent::new("responder", "city", "tenant");
        agent.has_agent_card = true;
        agent.last_heartbeat_at = Some(clock().now());
        agent
    }
    #[test]
    fn public_cards_cannot_approve_an_unadvertised_endpoint_skill_or_transport() {
        for (skill, endpoint, transport) in [
            ("execute-admin", "https://peer.example/a2a", "rest"),
            ("diagnose", "https://other.example/a2a", "rest"),
            ("diagnose", "https://peer.example/a2a", "grpc"),
            ("diagnose", "http://peer.example/a2a", "rest"),
            ("diagnose", "https://key:secret@peer.example/a2a", "rest"),
            ("diagnose", "https://peer.example/a2a?key=secret", "rest"),
        ] {
            assert!(
                ApprovedPeerBinding::new_trusted(
                    &card(),
                    target(),
                    skill,
                    endpoint,
                    transport,
                    effect()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn qualified_binding_digest_pins_actor_skill_and_the_complete_card() {
        let original = card();
        let digest = binding(&original).digest().to_owned();
        let mut changed = original.clone();
        changed.skills[0].description = Some("Untrusted planning instructions".into());
        assert_ne!(binding(&changed).digest(), digest);
        let alternate = PrincipalIdentity::new("other-actor", PrincipalKind::Agent).unwrap();
        assert_ne!(
            ApprovedPeerBinding::new_trusted(
                &original,
                alternate,
                "diagnose",
                "https://peer.example/a2a",
                "rest",
                effect()
            )
            .unwrap()
            .digest(),
            digest
        );
        let human = PrincipalIdentity::new("responder-actor", PrincipalKind::Human).unwrap();
        assert!(
            ApprovedPeerBinding::new_trusted(
                &original,
                human,
                "diagnose",
                "https://peer.example/a2a",
                "rest",
                effect()
            )
            .is_err()
        );
    }
    #[test]
    fn ambiguous_or_cross_scope_approvals_are_refused() {
        assert!(
            ApprovedPeerRegistry::new_trusted(
                Arc::new(MemoryStateStore::new()),
                "city",
                "tenant",
                vec![binding(&card()), binding(&card())]
            )
            .is_err()
        );
        assert!(
            ApprovedPeerRegistry::new_trusted(
                Arc::new(MemoryStateStore::new()),
                "other",
                "tenant",
                vec![binding(&card())]
            )
            .is_err()
        );
        let mut duplicate_skill = card();
        duplicate_skill.skills.push(Skill::new("diagnose"));
        assert!(
            ApprovedPeerBinding::new_trusted(
                &duplicate_skill,
                target(),
                "diagnose",
                "https://peer.example/a2a",
                "rest",
                effect()
            )
            .is_err()
        );
        let mut missing_resource = effect();
        missing_resource.resources.clear();
        assert!(
            ApprovedPeerBinding::new_trusted(
                &card(),
                target(),
                "diagnose",
                "https://peer.example/a2a",
                "rest",
                missing_resource
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn live_individual_record_must_match_the_operator_approved_card() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let original = card();
        let registry = ApprovedPeerRegistry::new_trusted(
            store.clone(),
            "city",
            "tenant",
            vec![binding(&original)],
        )
        .unwrap();
        let approved = &registry.bindings[&("responder".into(), "diagnose".into())];
        assert!(
            registry
                .inspect_binding(approved, &clock())
                .await
                .unwrap()
                .is_none()
        );
        publish(store.as_ref(), &agent(), &original).await;
        assert!(
            registry
                .inspect_binding(approved, &clock())
                .await
                .unwrap()
                .is_some()
        );
        let mut redirected = original.clone();
        redirected.interfaces[0].url = "https://redirected.example/a2a".into();
        publish(store.as_ref(), &agent(), &redirected).await;
        assert!(
            registry
                .inspect_binding(approved, &clock())
                .await
                .unwrap()
                .is_none()
        );
        publish(store.as_ref(), &agent(), &original).await;
        assert!(
            registry
                .inspect_binding(approved, &clock())
                .await
                .unwrap()
                .is_some()
        );
    }
    #[tokio::test]
    async fn administrative_state_and_liveness_are_independent_refusals() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let card = card();
        let registry = ApprovedPeerRegistry::new_trusted(
            store.clone(),
            "city",
            "tenant",
            vec![binding(&card)],
        )
        .unwrap();
        let approved = &registry.bindings[&("responder".into(), "diagnose".into())];
        for state in [AgentAdminState::Suspended, AgentAdminState::Banned] {
            let mut restricted = agent();
            restricted.admin_state = state;
            publish(store.as_ref(), &restricted, &card).await;
            assert!(
                registry
                    .inspect_binding(approved, &clock())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let mut unknown = agent();
        unknown.last_heartbeat_at = None;
        publish(store.as_ref(), &unknown, &card).await;
        assert!(
            registry
                .inspect_binding(approved, &clock())
                .await
                .unwrap()
                .is_none()
        );
        publish(store.as_ref(), &agent(), &card).await;
        let later = clock();
        later
            .advance_to(std::time::Duration::from_secs(61))
            .unwrap();
        assert!(
            registry
                .inspect_binding(approved, &later)
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn corrupt_or_wrong_scope_registry_records_are_not_candidates() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let original = card();
        let registry = ApprovedPeerRegistry::new_trusted(
            store.clone(),
            "city",
            "tenant",
            vec![binding(&original)],
        )
        .unwrap();
        let approved = &registry.bindings[&("responder".into(), "diagnose".into())];
        let mut forged = agent();
        forged.tenant = "other".into();
        publish(store.as_ref(), &forged, &original).await;
        assert!(matches!(
            registry.inspect_binding(approved, &clock()).await,
            Err(PeerDiscoveryError::Unavailable)
        ));
        store
            .set(
                &StateKey::new("city", "tenant", KeyKind::BusAgent, "responder"),
                "corrupt",
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            registry.inspect_binding(approved, &clock()).await,
            Err(PeerDiscoveryError::Unavailable)
        ));
    }
}
