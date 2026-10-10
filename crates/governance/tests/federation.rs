use std::sync::Arc;

use acteon_core::{
    PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef, TaskMessage, TaskRole,
};
use acteon_governance::{
    AttemptRequest, AttemptStatus, AuthorityChange, AuthorityCoordinator, CoordinationError,
    CoordinatorLimits, RootBudgetLimits, RootReservation, ScopePurpose, StartRegistration,
    context::AcceptedEffect,
    federation::{
        EvaluatedFederationTrustPublication, FederatedAttempt, FederatedDelegationImport,
        FederationCapabilityCard, FederationCapabilityRequirement, FederationCapabilitySigner,
        FederationEnvelope, FederationEnvelopeSigner, FederationGrantCeiling,
        FederationImportReference, FederationInvocation, FederationTrust,
        FederationTrustIssuanceCeiling, SignedFederationInvocation,
        verify_federation_capability_card,
    },
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use sha2::{Digest, Sha256};

fn actor(id: &str, kind: PrincipalKind) -> PrincipalIdentity {
    PrincipalIdentity::new(id, kind).unwrap()
}

fn at(ms: i64) -> ManualClock {
    ManualClock::new(chrono::DateTime::from_timestamp_millis(ms).unwrap())
}

fn agent() -> ResourceRef {
    ResourceRef::new(ResourceKind::Agent, "city", "tenant", "diagnostic").unwrap()
}

fn effect(operation: &str) -> AcceptedEffect {
    AcceptedEffect {
        operation: operation.into(),
        resources: vec![agent()],
    }
}

fn grant_ceiling() -> FederationGrantCeiling {
    FederationGrantCeiling {
        source: actor("foreign-investigator", PrincipalKind::Agent),
        target: actor("local-diagnostic", PrincipalKind::Agent),
        agent_resource: agent(),
        binding_digest: "a".repeat(64),
        skill: "diagnose".into(),
        ingress_effect: effect("agent.invoke"),
        effects: vec![effect("telemetry.read"), effect("artifact.write")],
        limits: RootBudgetLimits {
            max_units: 4,
            max_concurrent: 2,
            deadline_ms: 500,
        },
        max_depth: 2,
    }
}

fn trust(signer: &FederationEnvelopeSigner) -> FederationTrust {
    FederationTrust {
        id: "partner-a-key-2026".into(),
        revision: 1,
        foreign_domain: "partner-a.example".into(),
        local_audience: "acteon.city.example".into(),
        key_id: "partner-a-ed25519-1".into(),
        verifying_key: signer.verifying_key_hex(),
        minimum_issuer_epoch: 7,
        max_revocation_staleness_ms: 400,
        max_clock_skew_ms: 20,
        valid_from_ms: 0,
        deadline_ms: 1_000,
        approved: vec![grant_ceiling()],
    }
}

fn envelope() -> FederationEnvelope {
    let ceiling = grant_ceiling();
    FederationEnvelope::new(
        "foreign-job-1",
        "partner-a-key-2026",
        1,
        "partner-a.example",
        "acteon.city.example",
        "partner-a-ed25519-1",
        7,
        90,
        100,
        490,
        ceiling.source,
        ceiling.target,
        ceiling.agent_resource,
        ceiling.binding_digest,
        ceiling.skill,
        ceiling.ingress_effect,
        vec![effect("telemetry.read")],
        RootBudgetLimits {
            max_units: 2,
            max_concurrent: 1,
            deadline_ms: 450,
        },
        1,
    )
}

fn invocation(
    signer: &FederationEnvelopeSigner,
    import: &FederationImportReference,
    nonce: &str,
    digest_byte: char,
) -> SignedFederationInvocation {
    signer
        .sign_invocation(&FederationInvocation::new(
            nonce,
            import.clone(),
            TaskMessage::text(
                format!("message-{nonce}"),
                TaskRole::User,
                digest_byte.to_string().repeat(64),
            ),
            100,
            480,
        ))
        .unwrap()
}

fn capability_requirement() -> FederationCapabilityRequirement {
    FederationCapabilityRequirement {
        issuer_domain: "acteon.city.example".into(),
        audience: "partner-a.example".into(),
        minimum_issuer_epoch: 9,
        agent_resource: agent(),
        binding_digest: "a".repeat(64),
        skill: "diagnose".into(),
        protocol_revision: "acteon-federation-v1".into(),
        require_exact_replay: true,
        require_peer_finality: true,
        authentication_scheme: "bearer".into(),
    }
}

fn capability_card() -> FederationCapabilityCard {
    let requirement = capability_requirement();
    FederationCapabilityCard::new(
        &requirement.issuer_domain,
        &requirement.audience,
        "city-capability-key-1",
        9,
        100,
        500,
        requirement.agent_resource,
        requirement.binding_digest,
        vec!["diagnose".into(), "observe".into()],
        requirement.protocol_revision,
        true,
        true,
        vec!["bearer".into(), "mtls".into()],
    )
}

#[test]
fn signed_capability_card_negotiates_an_exact_peer_contract() {
    let signer = FederationCapabilitySigner::from_secret([22; 32]);
    let signed = signer.sign(&capability_card()).unwrap();
    let verified = verify_federation_capability_card(
        &signed,
        "city-capability-key-1",
        &signer.verifying_key_hex(),
        200,
        10,
        &capability_requirement(),
    )
    .unwrap();

    assert_eq!(verified.agent_resource, agent());
    assert!(verified.exact_replay_idempotent);
    assert!(verified.peer_finality);
    assert_eq!(signed, signer.sign(&verified).unwrap());
    assert_eq!(signed.digest().unwrap().len(), 64);
}

#[test]
fn capability_card_rejects_tampering_replay_and_missing_guarantees() {
    let signer = FederationCapabilitySigner::from_secret([23; 32]);
    let mut signed = signer.sign(&capability_card()).unwrap();
    signed.payload.push(' ');
    assert!(matches!(
        verify_federation_capability_card(
            &signed,
            "city-capability-key-1",
            &signer.verifying_key_hex(),
            200,
            10,
            &capability_requirement(),
        ),
        Err(CoordinationError::Restricted)
    ));

    let mut card = capability_card();
    card.audience = "another-source.example".into();
    let signed = signer.sign(&card).unwrap();
    assert!(matches!(
        verify_federation_capability_card(
            &signed,
            "city-capability-key-1",
            &signer.verifying_key_hex(),
            200,
            10,
            &capability_requirement(),
        ),
        Err(CoordinationError::Restricted)
    ));

    let mut card = capability_card();
    card.peer_finality = false;
    let signed = signer.sign(&card).unwrap();
    assert!(matches!(
        verify_federation_capability_card(
            &signed,
            "city-capability-key-1",
            &signer.verifying_key_hex(),
            200,
            10,
            &capability_requirement(),
        ),
        Err(CoordinationError::Restricted)
    ));

    let mut card = capability_card();
    card.issuer_epoch = 8;
    let signed = signer.sign(&card).unwrap();
    assert!(matches!(
        verify_federation_capability_card(
            &signed,
            "city-capability-key-1",
            &signer.verifying_key_hex(),
            200,
            10,
            &capability_requirement(),
        ),
        Err(CoordinationError::Restricted)
    ));

    let signed = signer.sign(&capability_card()).unwrap();
    assert!(matches!(
        verify_federation_capability_card(
            &signed,
            "city-capability-key-1",
            &signer.verifying_key_hex(),
            500,
            10,
            &capability_requirement(),
        ),
        Err(CoordinationError::Restricted)
    ));
}

async fn fixture() -> (
    Arc<MemoryStateStore>,
    AuthorityCoordinator,
    FederationEnvelopeSigner,
    ManualClock,
) {
    let store = Arc::new(MemoryStateStore::new());
    let coordinator = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    coordinator
        .reserve_scope(ScopePurpose::Execution)
        .await
        .unwrap();
    let signer = FederationEnvelopeSigner::from_secret([7; 32]);
    let policy = trust(&signer);
    let clock = at(100);
    let ceiling = FederationTrustIssuanceCeiling {
        issuer: actor("federation-admin", PrincipalKind::Human),
        approved: vec![policy.clone()],
        valid_from_ms: 0,
        deadline_ms: 1_000,
    };
    coordinator
        .publish_federation_trust(EvaluatedFederationTrustPublication {
            change_id: "trust-partner-a",
            trust: policy,
            expected_revision: 0,
            ceiling: &ceiling,
            evaluated_authority: &coordinator.snapshot().await.unwrap().stamp(),
            reason: "reviewed partner import",
            clock: &clock,
        })
        .await
        .unwrap();
    (store, coordinator, signer, clock)
}

async fn assert_generic_bypass_closed(
    coordinator: &AuthorityCoordinator,
    imported: &acteon_governance::federation::FederationImportRecord,
) {
    assert!(matches!(
        coordinator
            .register_attempt(AttemptRequest {
                id: "generic-bypass",
                subject: imported.target.id(),
                resources: &effect("telemetry.read").resources,
                request_digest: &"f".repeat(64),
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                reservation: Some(RootReservation {
                    root_id: imported.root_id.clone(),
                    units: 1,
                }),
                now_ms: 100,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
}

async fn assert_invalid_invocations_denied(
    coordinator: &AuthorityCoordinator,
    signer: &FederationEnvelopeSigner,
    reference: &FederationImportReference,
    clock: &ManualClock,
) {
    let mut tampered = invocation(signer, reference, "tampered", 'b');
    tampered.payload = tampered.payload.replace(&"b".repeat(64), &"c".repeat(64));
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &tampered,
                effect: &effect("telemetry.read"),
                request_digest: &"c".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
    let expired = signer
        .sign_invocation(&FederationInvocation::new(
            "expired",
            reference.clone(),
            TaskMessage::text("message-expired", TaskRole::User, "expired"),
            90,
            100,
        ))
        .unwrap();
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &expired,
                effect: &effect("telemetry.read"),
                request_digest: &"c".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
}

async fn assert_preflight_does_not_spend(
    coordinator: &AuthorityCoordinator,
    invocation: &SignedFederationInvocation,
    imported: &acteon_governance::federation::FederationImportRecord,
    clock: &ManualClock,
) {
    let before = coordinator.snapshot().await.unwrap();
    coordinator
        .check_federated_attempt(FederatedAttempt {
            invocation,
            effect: &effect("telemetry.read"),
            request_digest: &"b".repeat(64),
            ordinal: 0,
            units: 1,
            expected_authority: &before.stamp(),
            clock,
        })
        .await
        .unwrap();
    let after = coordinator.snapshot().await.unwrap();
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.starts, before.starts);
    assert_eq!(
        after.roots[&imported.root_id].spent_units,
        before.roots[&imported.root_id].spent_units
    );
}

#[tokio::test]
async fn audience_bound_import_is_attenuated_replay_safe_and_generic_path_closed() {
    let (_, coordinator, signer, clock) = fixture().await;
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    let imported = coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    assert_eq!(imported.effects, vec![effect("telemetry.read")]);
    assert_eq!(imported.limits.max_units, 2);

    let generation = coordinator.snapshot().await.unwrap().generation;
    let replay = coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    assert_eq!(replay, imported);
    assert_eq!(coordinator.snapshot().await.unwrap().generation, generation);

    let reference = FederationImportReference {
        id: imported.id.clone(),
        envelope_digest: imported.envelope_digest.clone(),
    };
    coordinator
        .check_federation_import(&reference, &clock)
        .await
        .unwrap();
    assert_generic_bypass_closed(&coordinator, &imported).await;
    let signed_invocation = invocation(&signer, &reference, "foreign-effect-1", 'b');
    assert_preflight_does_not_spend(&coordinator, &signed_invocation, &imported, &clock).await;
    let started = coordinator
        .register_federated_attempt(FederatedAttempt {
            invocation: &signed_invocation,
            effect: &effect("telemetry.read"),
            request_digest: &"b".repeat(64),
            ordinal: 0,
            units: 1,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    let StartRegistration::New(started) = started else {
        panic!("first signed invocation must register a new attempt");
    };
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &signed_invocation,
                effect: &effect("telemetry.read"),
                request_digest: &"b".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await
            .unwrap(),
        StartRegistration::Existing(_)
    ));

    assert_invalid_invocations_denied(&coordinator, &signer, &reference, &clock).await;
    assert_ne!(
        signed_invocation.attempt_id(0).unwrap(),
        signed_invocation.attempt_id(1).unwrap()
    );
    coordinator
        .settle(
            &signed_invocation.attempt_id(0).unwrap(),
            &started.token,
            AttemptStatus::Settled,
        )
        .await
        .unwrap();
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &signed_invocation,
                effect: &effect("telemetry.read"),
                request_digest: &"b".repeat(64),
                ordinal: 1,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await
            .unwrap(),
        StartRegistration::New(_)
    ));
}

#[tokio::test]
async fn revocation_blocks_new_effects_but_preserves_exact_start_observation() {
    let (_, coordinator, signer, clock) = fixture().await;
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    let imported = coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    let reference = FederationImportReference {
        id: imported.id,
        envelope_digest: imported.envelope_digest,
    };
    let first_invocation = invocation(&signer, &reference, "foreign-effect-1", 'b');
    coordinator
        .register_federated_attempt(FederatedAttempt {
            invocation: &first_invocation,
            effect: &effect("telemetry.read"),
            request_digest: &"b".repeat(64),
            ordinal: 0,
            units: 1,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();

    let policy = trust(&signer);
    let ceiling = FederationTrustIssuanceCeiling {
        issuer: actor("federation-admin", PrincipalKind::Human),
        approved: vec![policy],
        valid_from_ms: 0,
        deadline_ms: 1_000,
    };
    coordinator
        .revoke_federation_trust_evaluated(
            "revoke-partner-a",
            "partner-a-key-2026",
            1,
            &ceiling,
            &coordinator.snapshot().await.unwrap().stamp(),
            "partner key compromise",
            &clock,
        )
        .await
        .unwrap();
    assert!(matches!(
        coordinator
            .check_federation_import(&reference, &clock)
            .await,
        Err(CoordinationError::Restricted)
    ));
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &invocation(&signer, &reference, "foreign-effect-2", 'c'),
                effect: &effect("telemetry.read"),
                request_digest: &"c".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &first_invocation,
                effect: &effect("telemetry.read"),
                request_digest: &"b".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await
            .unwrap(),
        StartRegistration::Existing(_)
    ));
}

#[tokio::test]
async fn signature_audience_policy_replay_and_freshness_fail_closed() {
    let (_, coordinator, signer, clock) = fixture().await;

    let mut wrong_audience = envelope();
    wrong_audience.audience = "other-city.example".into();
    let foreign_assertion = signer.sign(&wrong_audience).unwrap();
    assert!(matches!(
        coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: &foreign_assertion,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));

    let attacker = FederationEnvelopeSigner::from_secret([9; 32]);
    let forged = attacker.sign(&envelope()).unwrap();
    assert!(matches!(
        coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: &forged,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));

    let mut expanded = envelope();
    expanded.effects.push(effect("production.remediate"));
    let expanded = signer.sign(&expanded).unwrap();
    assert!(matches!(
        coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: &expanded,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));

    let good = signer.sign(&envelope()).unwrap();
    coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &good,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    let mut changed = envelope();
    changed.effects = vec![effect("artifact.write")];
    let changed = signer.sign(&changed).unwrap();
    assert!(matches!(
        coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: &changed,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Conflict)
    ));

    let stale_clock = at(491);
    let mut stale = envelope();
    stale.id = "foreign-job-stale".into();
    let stale = signer.sign(&stale).unwrap();
    assert!(matches!(
        coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: &stale,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &stale_clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn trust_publication_cannot_retarget_domain_or_skip_revision() {
    let (_, coordinator, signer, clock) = fixture().await;
    let mut replacement = trust(&signer);
    replacement.revision = 2;
    replacement.foreign_domain = "attacker.example".into();
    let ceiling = FederationTrustIssuanceCeiling {
        issuer: actor("federation-admin", PrincipalKind::Human),
        approved: vec![replacement.clone()],
        valid_from_ms: 0,
        deadline_ms: 1_000,
    };
    assert!(matches!(
        coordinator
            .publish_federation_trust(EvaluatedFederationTrustPublication {
                change_id: "retarget",
                trust: replacement,
                expected_revision: 1,
                ceiling: &ceiling,
                evaluated_authority: &coordinator.snapshot().await.unwrap().stamp(),
                reason: "must fail",
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn generic_change_cannot_publish_or_revoke_federation_trust() {
    let (_, coordinator, signer, _) = fixture().await;
    assert!(matches!(
        coordinator
            .change(
                "generic-publish",
                AuthorityChange::PublishFederationTrust {
                    trust: trust(&signer),
                },
                "untrusted-caller",
                "bypass",
            )
            .await,
        Err(CoordinationError::Invalid(_))
    ));
    assert!(matches!(
        coordinator
            .change(
                "generic-revoke",
                AuthorityChange::RevokeFederationTrust {
                    trust_id: "partner-a-key-2026".into(),
                    expected_revision: 1,
                },
                "untrusted-caller",
                "bypass",
            )
            .await,
        Err(CoordinationError::Invalid(_))
    ));
}

#[tokio::test]
async fn key_rotation_invalidates_old_imports_and_accepts_the_new_revision() {
    let (_, coordinator, signer, clock) = fixture().await;
    let old_assertion = signer.sign(&envelope()).unwrap();
    let old_import = coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &old_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    let old_reference = FederationImportReference {
        id: old_import.id,
        envelope_digest: old_import.envelope_digest,
    };

    let next_signer = FederationEnvelopeSigner::from_secret([8; 32]);
    let mut next_trust = trust(&next_signer);
    next_trust.revision = 2;
    next_trust.key_id = "partner-a-ed25519-2".into();
    let ceiling = FederationTrustIssuanceCeiling {
        issuer: actor("federation-admin", PrincipalKind::Human),
        approved: vec![next_trust.clone()],
        valid_from_ms: 0,
        deadline_ms: 1_000,
    };
    coordinator
        .publish_federation_trust(EvaluatedFederationTrustPublication {
            change_id: "rotate-partner-a",
            trust: next_trust,
            expected_revision: 1,
            ceiling: &ceiling,
            evaluated_authority: &coordinator.snapshot().await.unwrap().stamp(),
            reason: "scheduled key rotation",
            clock: &clock,
        })
        .await
        .unwrap();
    assert!(matches!(
        coordinator
            .check_federation_import(&old_reference, &clock)
            .await,
        Err(CoordinationError::Restricted)
    ));

    let mut next_envelope = envelope();
    next_envelope.id = "foreign-job-2".into();
    next_envelope.trust_revision = 2;
    next_envelope.key_id = "partner-a-ed25519-2".into();
    let next_assertion = next_signer.sign(&next_envelope).unwrap();
    coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &next_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn local_target_revocation_denies_import_without_allocating_a_root() {
    let (_, coordinator, signer, clock) = fixture().await;
    coordinator
        .change(
            "revoke-local-target",
            AuthorityChange::RevokeSubject {
                subject: "local-diagnostic".into(),
            },
            "operator",
            "local offboarding",
        )
        .await
        .unwrap();
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    assert!(matches!(
        coordinator
            .import_federated_delegation(FederatedDelegationImport {
                signed: &foreign_assertion,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
    let snapshot = coordinator.snapshot().await.unwrap();
    assert!(snapshot.federation_imports.is_empty());
    assert!(snapshot.roots.is_empty());
}

#[tokio::test]
async fn retained_signed_evidence_detects_import_tampering_after_restart() {
    let (store, coordinator, signer, clock) = fixture().await;
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
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
    raw["federation_imports"]["foreign-job-1"]["effects"][0]["operation"] =
        "production.remediate".into();
    store.set(&key, &raw.to_string(), None).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store, "city", "tenant")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn reconstruction_rejects_validly_signed_envelope_that_local_policy_never_accepted() {
    let (store, coordinator, signer, clock) = fixture().await;
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    let imported = coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    let mut invalid = envelope();
    invalid.audience = "attacker.example".into();
    let invalid = signer.sign(&invalid).unwrap();
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&invalid).unwrap())
    );
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let mut raw: serde_json::Value =
        serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
    let root = raw["roots"]
        .as_object_mut()
        .unwrap()
        .remove(&imported.root_id)
        .unwrap();
    let record = &mut raw["federation_imports"]["foreign-job-1"];
    record["signed_envelope"] = serde_json::to_value(invalid).unwrap();
    record["envelope_digest"] = digest.clone().into();
    record["root_id"] = format!("federation:{digest}").into();
    raw["roots"]
        .as_object_mut()
        .unwrap()
        .insert(format!("federation:{digest}"), root);
    store.set(&key, &raw.to_string(), None).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store, "city", "tenant")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn reconstruction_rejects_import_claiming_acceptance_after_local_revocation() {
    let (store, coordinator, signer, clock) = fixture().await;
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    coordinator
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    coordinator
        .change(
            "revoke-imported-target",
            AuthorityChange::RevokeSubject {
                subject: "local-diagnostic".into(),
            },
            "operator",
            "local offboarding",
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
    raw["federation_imports"]["foreign-job-1"]["authority"]["generation"] =
        raw["generation"].clone();
    store.set(&key, &raw.to_string(), None).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store, "city", "tenant")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn protocol_twelve_requires_explicit_reviewed_cutover() {
    let store = Arc::new(MemoryStateStore::new());
    let coordinator = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    coordinator
        .reserve_scope(ScopePurpose::Execution)
        .await
        .unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let mut raw = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
    raw["schema_version"] = 12.into();
    raw.as_object_mut().unwrap().remove("federation_trusts");
    raw.as_object_mut().unwrap().remove("federation_imports");
    store.set(&key, &raw.to_string(), None).await.unwrap();
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
        "enable federation protocol",
    )
    .await
    .unwrap();
    assert_eq!(plan.report().from_protocol, 12);
    assert_eq!(plan.report().to_protocol, 13);
    plan.apply(&plan.report().review_digest).await.unwrap();
    let upgraded = AuthorityCoordinator::connect(store, "city", "tenant")
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert!(upgraded.federation_trusts.is_empty());
    assert!(upgraded.federation_imports.is_empty());
}

async fn federation_backend_contract(first: Arc<dyn StateStore>, second: Arc<dyn StateStore>) {
    let coordinator =
        AuthorityCoordinator::initialize(first, "city", "tenant", CoordinatorLimits::default())
            .await
            .unwrap();
    coordinator
        .reserve_scope(ScopePurpose::Execution)
        .await
        .unwrap();
    let replica = AuthorityCoordinator::connect(second, "city", "tenant")
        .await
        .unwrap();
    let signer = FederationEnvelopeSigner::from_secret([17; 32]);
    let policy = trust(&signer);
    let clock = at(100);
    let ceiling = FederationTrustIssuanceCeiling {
        issuer: actor("federation-admin", PrincipalKind::Human),
        approved: vec![policy.clone()],
        valid_from_ms: 0,
        deadline_ms: 1_000,
    };
    coordinator
        .publish_federation_trust(EvaluatedFederationTrustPublication {
            change_id: "trust-partner-a",
            trust: policy,
            expected_revision: 0,
            ceiling: &ceiling,
            evaluated_authority: &coordinator.snapshot().await.unwrap().stamp(),
            reason: "reviewed partner import",
            clock: &clock,
        })
        .await
        .unwrap();
    let foreign_assertion = signer.sign(&envelope()).unwrap();
    let imported = replica
        .import_federated_delegation(FederatedDelegationImport {
            signed: &foreign_assertion,
            expected_authority: &replica.snapshot().await.unwrap().stamp(),
            clock: &clock,
        })
        .await
        .unwrap();
    let reference = FederationImportReference {
        id: imported.id,
        envelope_digest: imported.envelope_digest,
    };
    let cross_replica = invocation(&signer, &reference, "cross-replica-effect", 'd');
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &cross_replica,
                effect: &effect("telemetry.read"),
                request_digest: &"d".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await
            .unwrap(),
        StartRegistration::New(_)
    ));
    replica
        .revoke_federation_trust_evaluated(
            "revoke-partner-a",
            "partner-a-key-2026",
            1,
            &ceiling,
            &replica.snapshot().await.unwrap().stamp(),
            "partner trust withdrawn",
            &clock,
        )
        .await
        .unwrap();
    assert!(matches!(
        coordinator
            .register_federated_attempt(FederatedAttempt {
                invocation: &invocation(&signer, &reference, "blocked-after-revocation", 'e'),
                effect: &effect("telemetry.read"),
                request_digest: &"e".repeat(64),
                ordinal: 0,
                units: 1,
                expected_authority: &coordinator.snapshot().await.unwrap().stamp(),
                clock: &clock,
            })
            .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; independent federation replicas"]
async fn independent_redis_federation_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("federation-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    federation_backend_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        Arc::new(RedisStateStore::new(&config).unwrap()),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL; independent federation replicas"]
async fn independent_postgres_federation_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("federation{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    federation_backend_contract(
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
    )
    .await;
    let pool = sqlx::PgPool::connect(&config.url).await.unwrap();
    for suffix in ["state", "locks", "timeout_index", "chain_ready_index"] {
        sqlx::query(&format!(
            "DROP TABLE public.{}{suffix}",
            config.table_prefix
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires DYNAMODB_ENDPOINT; independent federation replicas"]
async fn independent_dynamodb_federation_contract() {
    use acteon_state_dynamodb::{DynamoConfig, DynamoStateStore, build_client, create_table};
    let config = DynamoConfig {
        endpoint_url: Some(std::env::var("DYNAMODB_ENDPOINT").unwrap()),
        table_name: format!("federation_{}", uuid::Uuid::new_v4().simple()),
        key_prefix: format!("federation_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let client = build_client(&config).await;
    create_table(&client, &config.table_name).await.unwrap();
    federation_backend_contract(
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
    )
    .await;
    client
        .delete_table()
        .table_name(&config.table_name)
        .send()
        .await
        .unwrap();
}
