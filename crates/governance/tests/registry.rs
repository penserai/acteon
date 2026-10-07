use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, CoordinationError, CoordinatorLimits, ScopePurpose,
    control::{ControlChangeAuthorization, ControlChangeCeiling},
    registry::{AgentRegistryIssuanceCeiling, AgentRegistryQualification},
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use std::{collections::BTreeMap, sync::Arc};
fn actor(id: &str, kind: PrincipalKind) -> PrincipalIdentity {
    PrincipalIdentity::new(id, kind).unwrap()
}
fn qualification(revision: u64, digest: char) -> AgentRegistryQualification {
    AgentRegistryQualification {
        agent: ResourceRef::new(ResourceKind::Agent, "city", "tenant", "worker").unwrap(),
        target: actor("worker", PrincipalKind::Agent),
        revision,
        bindings: BTreeMap::from([("work".into(), digest.to_string().repeat(64))]),
    }
}
fn clock() -> ManualClock {
    ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap())
}
async fn fixture() -> (Arc<MemoryStateStore>, AuthorityCoordinator) {
    let store = Arc::new(MemoryStateStore::new());
    let c = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    c.reserve_scope(ScopePurpose::Execution).await.unwrap();
    (store, c)
}
async fn publish(
    c: &AuthorityCoordinator,
    q: AgentRegistryQualification,
    id: &str,
) -> Result<acteon_governance::ChangeRecord, CoordinationError> {
    let ceiling = AgentRegistryIssuanceCeiling {
        issuer: actor("operator", PrincipalKind::Human),
        approved: vec![q.clone()],
        valid_from_ms: 0,
        deadline_ms: 1000,
    };
    c.publish_agent_registry(
        id,
        q.clone(),
        q.revision - 1,
        &ceiling,
        &c.snapshot().await.unwrap().stamp(),
        "reviewed",
        &clock(),
    )
    .await
}
#[tokio::test]
async fn qualification_replay_retirement_and_replacement_preserve_monotonic_history() {
    let (store, c) = fixture().await;
    let q = qualification(1, 'a');
    publish(&c, q.clone(), "qualify-1").await.unwrap();
    let generation = c.snapshot().await.unwrap().generation;
    publish(&c, q.clone(), "qualify-1").await.unwrap();
    assert_eq!(generation, c.snapshot().await.unwrap().generation);
    let ceiling = ControlChangeCeiling {
        actor: actor("operator", PrincipalKind::Human),
        subjects: vec![],
        resources: vec![q.agent.clone()],
        valid_from_ms: 0,
        deadline_ms: 1000,
    };
    let stamp = c.snapshot().await.unwrap().stamp();
    c.change_evaluated(
        "retire",
        AuthorityChange::RetireAgentRegistry {
            agent: q.agent,
            expected_revision: 1,
        },
        "card changed",
        ControlChangeAuthorization {
            ceiling: &ceiling,
            evaluated_authority: &stamp,
            clock: &clock(),
        },
    )
    .await
    .unwrap();
    assert!(c.snapshot().await.unwrap().agent_registry["worker"].retired);
    assert!(publish(&c, qualification(2, 'a'), "reuse").await.is_err());
    publish(&c, qualification(2, 'b'), "qualify-2")
        .await
        .unwrap();
    let restarted = AuthorityCoordinator::connect(store, "city", "tenant")
        .await
        .unwrap();
    let record = &restarted.snapshot().await.unwrap().agent_registry["worker"];
    assert!(!record.retired);
    assert_eq!(record.qualification.revision, 2);
}
#[tokio::test]
async fn publication_requires_independent_exact_approval_and_current_authority() {
    let (_, c) = fixture().await;
    let q = qualification(1, 'a');
    let ceiling = AgentRegistryIssuanceCeiling {
        issuer: actor("operator", PrincipalKind::Human),
        approved: vec![qualification(1, 'b')],
        valid_from_ms: 0,
        deadline_ms: 1000,
    };
    let stamp = c.snapshot().await.unwrap().stamp();
    assert!(
        c.publish_agent_registry(
            "wrong",
            q.clone(),
            0,
            &ceiling,
            &stamp,
            "reviewed",
            &clock()
        )
        .await
        .is_err()
    );
    assert!(
        c.change(
            "bypass",
            AuthorityChange::PublishAgentRegistry {
                qualification: q.clone()
            },
            "operator",
            "reviewed"
        )
        .await
        .is_err()
    );
    let ceiling = AgentRegistryIssuanceCeiling {
        approved: vec![q.clone()],
        ..ceiling
    };
    c.change(
        "close",
        AuthorityChange::CloseResource {
            resource: q.agent.clone(),
        },
        "operator",
        "closed",
    )
    .await
    .unwrap();
    assert!(matches!(
        c.publish_agent_registry("stale", q, 0, &ceiling, &stamp, "reviewed", &clock())
            .await,
        Err(CoordinationError::StaleAuthority)
    ));
    assert!(c.snapshot().await.unwrap().agent_registry.is_empty());
}
#[tokio::test]
async fn altered_projection_is_rejected_instead_of_resurrecting_a_retired_binding() {
    let (store, c) = fixture().await;
    let q = qualification(1, 'a');
    publish(&c, q.clone(), "qualify").await.unwrap();
    c.change(
        "retire",
        AuthorityChange::RetireAgentRegistry {
            agent: q.agent,
            expected_revision: 1,
        },
        "operator",
        "retired",
    )
    .await
    .unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let mut raw: serde_json::Value =
        serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
    raw["agent_registry"]["worker"]["retired"] = false.into();
    store
        .set(&key, &serde_json::to_string(&raw).unwrap(), None)
        .await
        .unwrap();
    assert!(c.snapshot().await.is_err());
}
#[tokio::test]
async fn explicit_protocol_10_upgrade_retains_state_and_rejects_implicit_adoption() {
    let (store, c) = fixture().await;
    let before = c.snapshot().await.unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let mut raw = serde_json::to_value(&before).unwrap();
    raw["schema_version"] = 10.into();
    raw.as_object_mut().unwrap().remove("agent_registry");
    store
        .set(&key, &serde_json::to_string(&raw).unwrap(), None)
        .await
        .unwrap();
    assert!(
        AuthorityCoordinator::connect(store.clone(), "city", "tenant")
            .await
            .is_err()
    );
    let plan = AuthorityCoordinator::plan_scope_upgrade(
        store.clone(),
        "city",
        "tenant",
        ScopePurpose::Execution,
        "operator",
        "registry fence upgrade",
    )
    .await
    .unwrap();
    assert_eq!(plan.report().from_protocol, 10);
    assert_eq!(plan.report().to_protocol, 11);
    assert!(plan.apply(&plan.report().review_digest).await.unwrap());
    let after = AuthorityCoordinator::connect(store, "city", "tenant")
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(before.incarnation, after.incarnation);
    assert!(after.agent_registry.is_empty());
    assert_eq!(before.budget_parents, after.budget_parents);
    assert_eq!(before.roots.len(), after.roots.len());
}
