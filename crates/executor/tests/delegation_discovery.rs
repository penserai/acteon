//! Real `StateStore` registry reads composed with independently credentialed authority.
use std::sync::Arc;

use acteon_core::{Agent, AgentCard, PrincipalIdentity, Skill, bus_agent_card::Interface};
use acteon_executor::delegation::{
    ApprovedPeerBinding, ApprovedPeerRegistry, PeerCandidate, PeerCandidateQuery,
    PeerDiscoveryError, PeerRecipientResolver, RecipientDiscoveryContext,
};
use acteon_governance::{
    AuthorityChange, context::VerifiedExecutionContext, permit::PermitReference,
};
use acteon_state::{
    KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, ReadOperation},
};
use acteon_state_memory::MemoryStateStore;
use acteon_time::Clock;

#[path = "../../governance/tests/common/delegation.rs"]
mod fixture;
use fixture::{Fixture, actor, capture, effect, permits};

struct Resolver {
    context: VerifiedExecutionContext,
    permits: Vec<PermitReference>,
}
#[async_trait::async_trait]
impl PeerRecipientResolver for Resolver {
    async fn resolve(
        &self,
        _: &str,
        _: &str,
        _: &PrincipalIdentity,
    ) -> Result<Option<RecipientDiscoveryContext>, PeerDiscoveryError> {
        Ok(Some(RecipientDiscoveryContext {
            context: self.context.clone(),
            permits: self.permits.clone(),
        }))
    }
}
fn card() -> AgentCard {
    let mut card = AgentCard::new("responder", "city", "tenant", "Responder", "v1");
    let mut skill = Skill::new("diagnose");
    skill.description = Some("Diagnose an incident; this description is untrusted data".into());
    card.skills.push(skill);
    card.interfaces.push(Interface {
        kind: "rest".into(),
        url: "https://peer.example/a2a".into(),
    });
    card
}
async fn registry(
    store: Arc<dyn StateStore>,
    fixture: &Fixture,
) -> (ApprovedPeerRegistry, AgentCard) {
    let card = card();
    let mut agent = Agent::new("responder", "city", "tenant");
    agent.has_agent_card = true;
    agent.last_heartbeat_at = Some(fixture.clock.now());
    store
        .set(
            &StateKey::new("city", "tenant", KeyKind::BusAgent, "responder"),
            &serde_json::to_string(&agent).unwrap(),
            None,
        )
        .await
        .unwrap();
    store
        .set(
            &StateKey::new("city", "tenant", KeyKind::BusAgentCard, "responder"),
            &serde_json::to_string(&card).unwrap(),
            None,
        )
        .await
        .unwrap();
    let binding = ApprovedPeerBinding::new_trusted(
        &card,
        actor("responder"),
        "diagnose",
        "https://peer.example/a2a",
        "rest",
        effect(),
    )
    .unwrap();
    (
        ApprovedPeerRegistry::new_trusted(store, "city", "tenant", vec![binding]).unwrap(),
        card,
    )
}
async fn discover(
    registry: &ApprovedPeerRegistry,
    fixture: &Fixture,
    resolver: &Resolver,
) -> Result<Vec<PeerCandidate>, PeerDiscoveryError> {
    registry
        .discover_candidates(
            "diagnose",
            PeerCandidateQuery {
                coordinator: &fixture.coordinator,
                parent: &fixture.parent,
                parent_permits: &permits("planner"),
                recipients: resolver,
                clock: &fixture.clock,
            },
        )
        .await
}
#[tokio::test]
async fn approved_registry_candidates_require_both_credentials_without_spending() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let fixture = fixture::fixture_with_store(store.clone()).await;
    let (registry, _) = registry(store, &fixture).await;
    let resolver = Resolver {
        context: fixture.recipient.clone(),
        permits: permits("responder"),
    };
    let before = serde_json::to_value(fixture.coordinator.snapshot().await.unwrap()).unwrap();
    let candidates = discover(&registry, &fixture, &resolver).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].target, actor("responder"));
    assert_eq!(candidates[0].agent_id, "responder");
    assert_eq!(candidates[0].skill, "diagnose");
    assert_eq!(candidates[0].binding_digest.len(), 64);
    assert_eq!(candidates[0].description, card().skills[0].description);
    assert_eq!(
        before,
        serde_json::to_value(fixture.coordinator.snapshot().await.unwrap()).unwrap()
    );
    fixture
        .coordinator
        .change(
            "closure",
            AuthorityChange::CloseResource {
                resource: effect().resources[0].clone(),
            },
            "operator",
            "incident closure",
        )
        .await
        .unwrap();
    assert!(
        discover(&registry, &fixture, &resolver)
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn registry_edits_and_resolver_identity_substitution_cannot_redirect_discovery() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let fixture = fixture::fixture_with_store(store.clone()).await;
    let (registry, mut card) = registry(store.clone(), &fixture).await;
    let forged = Resolver {
        context: fixture.parent.clone(),
        permits: permits("planner"),
    };
    assert!(
        discover(&registry, &fixture, &forged)
            .await
            .unwrap()
            .is_empty()
    );
    let uncredentialed = capture(
        &fixture.coordinator,
        &fixture.contexts,
        "responder",
        &fixture.clock,
        false,
    )
    .await;
    let forged = Resolver {
        context: uncredentialed,
        permits: permits("responder"),
    };
    assert!(
        discover(&registry, &fixture, &forged)
            .await
            .unwrap()
            .is_empty()
    );
    let resolver = Resolver {
        context: fixture.recipient.clone(),
        permits: permits("responder"),
    };
    card.interfaces[0].url = "https://different.example/a2a".into();
    store
        .set(
            &StateKey::new("city", "tenant", KeyKind::BusAgentCard, "responder"),
            &serde_json::to_string(&card).unwrap(),
            None,
        )
        .await
        .unwrap();
    assert!(
        discover(&registry, &fixture, &resolver)
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn authority_expiry_during_registry_read_is_resampled_before_disclosure() {
    let store = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let fixture = Arc::new(fixture::fixture_with_store(store.clone()).await);
    let (registry, _) = registry(store.clone(), &fixture).await;
    let (reached, resume) = store
        .pause_next_read(
            KeyKind::BusAgentCard,
            ReadOperation::Get,
            FaultTiming::After,
        )
        .unwrap();
    let reading = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            let resolver = Resolver {
                context: fixture.recipient.clone(),
                permits: permits("responder"),
            };
            discover(&registry, &fixture, &resolver).await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), reached)
        .await
        .unwrap()
        .unwrap();
    fixture
        .clock
        .advance_to(std::time::Duration::from_secs(20))
        .unwrap();
    resume.send(()).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), reading)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .coordinator
            .snapshot()
            .await
            .unwrap()
            .starts
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; independent registry and authority clients"]
async fn independent_redis_clients_discover_only_current_credentialed_peers() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("peer-discovery-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let original: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let peer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let mut fixture = fixture::fixture_with_store(original.clone()).await;
    let (registry, _) = registry(peer.clone(), &fixture).await;
    let resolver = Resolver {
        context: fixture.recipient.clone(),
        permits: permits("responder"),
    };
    fixture.coordinator =
        acteon_governance::AuthorityCoordinator::connect(peer.clone(), "city", "tenant")
            .await
            .unwrap();
    assert_eq!(
        discover(&registry, &fixture, &resolver)
            .await
            .unwrap()
            .len(),
        1
    );
    let writer = acteon_governance::AuthorityCoordinator::connect(original, "city", "tenant")
        .await
        .unwrap();
    writer
        .change(
            "revoke",
            AuthorityChange::RevokeSubject {
                subject: "responder".into(),
            },
            "operator",
            "revoke peer",
        )
        .await
        .unwrap();
    assert!(
        discover(&registry, &fixture, &resolver)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .coordinator
            .snapshot()
            .await
            .unwrap()
            .starts
            .is_empty()
    );
    // Delete only this test's explicitly named records under its UUID prefix.
    for (kind, id) in [
        (
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            "authority".into(),
        ),
        (
            KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
            fixture.parent.reference().unwrap().context_id().to_string(),
        ),
        (
            KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
            fixture
                .recipient
                .reference()
                .unwrap()
                .context_id()
                .to_string(),
        ),
        (KeyKind::BusAgent, "responder".into()),
        (KeyKind::BusAgentCard, "responder".into()),
    ] {
        assert!(
            peer.delete(&StateKey::new("city", "tenant", kind, id))
                .await
                .unwrap()
        );
    }
}
