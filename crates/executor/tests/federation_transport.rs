use std::time::Duration;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
};

use acteon_core::{
    ExecutionContextReference, FederationImportReceipt, PrincipalIdentity, PrincipalKind,
    ResourceKind, ResourceRef, Task, TaskMessage, TaskRole,
};
use acteon_executor::federation_finality::{
    FederationFinalityVerifier, FederationPeerFinality, HmacFederationFinalityVerifier,
    sign_federation_finality_receipt,
};
use acteon_executor::federation_transport::{
    DurableFederationTransport, FEDERATION_DELIVERY_KIND, FederationCapabilityDisposition,
    FederationCapabilityEvidence, FederationCapabilityRequest, FederationDeliveryAbandonment,
    FederationDeliveryEvidenceReader, FederationDeliveryEvidenceStatus,
    FederationDeliveryFinalityReconciler, FederationDeliveryIntent, FederationDeliveryReconciler,
    FederationDeliveryStatus, FederationFinalityAcceptance, FederationImportDisposition,
    FederationImportRequest, FederationInvokeDisposition, FederationInvokeRequest,
    FederationTransportAdapter, FederationTransportDependencies, FederationTransportError,
};
use acteon_governance::{
    AuthorityStamp, RootBudgetLimits,
    context::AcceptedEffect,
    federation::{
        FederationEnvelope, FederationEnvelopeSigner, FederationImportReference,
        FederationInvocation, SignedFederationEnvelope, SignedFederationInvocation,
    },
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::{Clock, ManualClock};
use async_trait::async_trait;
use tokio::sync::Notify;

const ACCEPT: u8 = 0;
const UNCERTAIN: u8 = 1;
const REJECT: u8 = 2;

struct Adapter {
    state: Arc<dyn StateStore>,
    replay_safe: bool,
    capability_mode: AtomicU8,
    import_mode: AtomicU8,
    invoke_mode: AtomicU8,
    imports: AtomicUsize,
    invokes: AtomicUsize,
    negotiations: AtomicUsize,
    saw_persisted_intent: AtomicBool,
    block_capability: AtomicBool,
    capability_started: Notify,
    release_capability: Notify,
    block_import: AtomicBool,
    import_started: Notify,
    release_import: Notify,
}

impl Adapter {
    fn new(state: Arc<dyn StateStore>, replay_safe: bool) -> Self {
        Self {
            state,
            replay_safe,
            capability_mode: AtomicU8::new(ACCEPT),
            import_mode: AtomicU8::new(ACCEPT),
            invoke_mode: AtomicU8::new(ACCEPT),
            imports: AtomicUsize::new(0),
            invokes: AtomicUsize::new(0),
            negotiations: AtomicUsize::new(0),
            saw_persisted_intent: AtomicBool::new(false),
            block_capability: AtomicBool::new(false),
            capability_started: Notify::new(),
            release_capability: Notify::new(),
            block_import: AtomicBool::new(false),
            import_started: Notify::new(),
            release_import: Notify::new(),
        }
    }

    async fn persisted(&self) -> bool {
        self.state
            .scan_keys(
                "local",
                "tenant",
                KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
                None,
            )
            .await
            .is_ok_and(|entries| !entries.is_empty())
    }
}

#[async_trait]
impl FederationTransportAdapter for Adapter {
    fn revision(&self) -> &'static str {
        "federation-http-v1"
    }

    fn binding_digest(&self) -> &'static str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn exact_replay_idempotent(&self) -> bool {
        self.replay_safe
    }

    async fn negotiate(
        &self,
        _request: FederationCapabilityRequest<'_>,
    ) -> Result<FederationCapabilityDisposition, FederationTransportError> {
        self.negotiations.fetch_add(1, Ordering::SeqCst);
        if self.block_capability.load(Ordering::SeqCst) {
            self.capability_started.notify_one();
            self.release_capability.notified().await;
        }
        match self.capability_mode.load(Ordering::SeqCst) {
            UNCERTAIN => Ok(FederationCapabilityDisposition::Unavailable),
            REJECT => Ok(FederationCapabilityDisposition::Rejected {
                code: "peer_capability_denied".into(),
            }),
            _ => Ok(FederationCapabilityDisposition::Accepted(
                FederationCapabilityEvidence {
                    card_digest: "c".repeat(64),
                    key_id: "peer-card-key-1".into(),
                    issuer_epoch: 1,
                    expires_at_ms: 10_000,
                    protocol_revision: "acteon-federation-v1".into(),
                    authentication_scheme: "bearer".into(),
                },
            )),
        }
    }

    async fn import(
        &self,
        request: FederationImportRequest<'_>,
    ) -> Result<FederationImportDisposition, FederationTransportError> {
        self.imports.fetch_add(1, Ordering::SeqCst);
        self.saw_persisted_intent
            .store(self.persisted().await, Ordering::SeqCst);
        if self.block_import.load(Ordering::SeqCst) {
            self.import_started.notify_one();
            self.release_import.notified().await;
        }
        match self.import_mode.load(Ordering::SeqCst) {
            UNCERTAIN => Ok(FederationImportDisposition::Uncertain),
            REJECT => Ok(FederationImportDisposition::Rejected {
                code: "remote_trust_denied".into(),
            }),
            _ => {
                let envelope = request
                    .envelope
                    .envelope()
                    .map_err(|_| FederationTransportError::Invalid)?;
                Ok(FederationImportDisposition::Accepted(Box::new(
                    FederationImportReceipt {
                        namespace: envelope.agent_resource.namespace().into(),
                        tenant: envelope.agent_resource.tenant().into(),
                        import_id: envelope.id,
                        envelope_digest: request
                            .envelope
                            .digest()
                            .map_err(|_| FederationTransportError::Invalid)?,
                        trust_id: envelope.trust_id,
                        trust_revision: envelope.trust_revision,
                        issuer_domain: envelope.issuer_domain,
                        source: envelope.source,
                        target: envelope.target,
                        agent_resource: envelope.agent_resource,
                        binding_digest: envelope.binding_digest,
                        skill: envelope.skill,
                        root_id: "remote-root-1".into(),
                        expires_at_ms: envelope.expires_at_ms,
                        accepted_at_ms: 110,
                        execution_started: false,
                    },
                )))
            }
        }
    }

    async fn invoke(
        &self,
        request: FederationInvokeRequest<'_>,
    ) -> Result<FederationInvokeDisposition, FederationTransportError> {
        self.invokes.fetch_add(1, Ordering::SeqCst);
        match self.invoke_mode.load(Ordering::SeqCst) {
            UNCERTAIN => Ok(FederationInvokeDisposition::Uncertain),
            REJECT => Ok(FederationInvokeDisposition::Rejected {
                code: "remote_execution_denied".into(),
            }),
            _ => {
                let invocation = request
                    .invocation
                    .invocation()
                    .map_err(|_| FederationTransportError::Invalid)?;
                let mut task = Task::new_at(
                    request.invocation.digest().unwrap()[..32].to_owned(),
                    "remote",
                    "tenant",
                    chrono::DateTime::from_timestamp_millis(110).unwrap(),
                );
                let mut message = request.message.clone();
                message.task_id = Some(task.id.clone());
                task.append_history_at(
                    message,
                    chrono::DateTime::from_timestamp_millis(110).unwrap(),
                )
                .map_err(|_| FederationTransportError::Unavailable)?;
                if serde_json::to_value(invocation.message).unwrap()
                    != serde_json::to_value(request.message).unwrap()
                {
                    return Err(FederationTransportError::Conflict);
                }
                Ok(FederationInvokeDisposition::Accepted(Box::new(task)))
            }
        }
    }
}

struct Fixture {
    state: Arc<MemoryStateStore>,
    clock: Arc<ManualClock>,
    adapter: Arc<Adapter>,
    transport: DurableFederationTransport,
    parent: ExecutionContextReference,
    reconciliation_resource: ResourceRef,
    envelope: SignedFederationEnvelope,
    invocation: SignedFederationInvocation,
    message: TaskMessage,
}

impl Fixture {
    fn new(replay_safe: bool) -> Self {
        let state = Arc::new(MemoryStateStore::new());
        let state_dyn: Arc<dyn StateStore> = state.clone();
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp_millis(100).unwrap(),
        ));
        let adapter = Arc::new(Adapter::new(state_dyn.clone(), replay_safe));
        let source = PrincipalIdentity::new("source-agent", PrincipalKind::Agent).unwrap();
        let target = PrincipalIdentity::new("remote-agent", PrincipalKind::Agent).unwrap();
        let parent = ExecutionContextReference::new(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "local".into(),
            "tenant".into(),
            source.clone(),
            "b".repeat(64),
        )
        .unwrap();
        let agent = ResourceRef::new(ResourceKind::Agent, "remote", "tenant", "responder").unwrap();
        let ingress = AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: vec![agent.clone()],
        };
        let direct = AcceptedEffect {
            operation: "incident.inspect".into(),
            resources: vec![agent.clone()],
        };
        let signer = FederationEnvelopeSigner::from_secret([31; 32]);
        let envelope = signer
            .sign(&FederationEnvelope::new(
                "export-1",
                "remote-trust-1",
                1,
                "local.example",
                "remote.example",
                "local-ed25519-1",
                1,
                90,
                100,
                5_000,
                source,
                target,
                agent,
                "c".repeat(64),
                "respond",
                ingress,
                vec![direct],
                RootBudgetLimits {
                    max_units: 2,
                    max_concurrent: 1,
                    deadline_ms: 4_900,
                },
                1,
            ))
            .unwrap();
        let message = TaskMessage::text("message-1", TaskRole::User, "inspect incident");
        let invocation = signer
            .sign_invocation(&FederationInvocation::new(
                "invocation-1",
                FederationImportReference {
                    id: "export-1".into(),
                    envelope_digest: envelope.digest().unwrap(),
                },
                message.clone(),
                100,
                4_000,
            ))
            .unwrap();
        let transport = DurableFederationTransport::new_trusted(
            FederationTransportDependencies {
                state: state_dyn,
                clock: clock.clone(),
                encryptor: None,
            },
            adapter.clone(),
            Duration::from_secs(1),
        )
        .unwrap();
        Self {
            state,
            clock,
            adapter,
            transport,
            parent,
            reconciliation_resource: ResourceRef::new(
                ResourceKind::ExternalService,
                "local",
                "tenant",
                "remote-triage",
            )
            .unwrap(),
            envelope,
            invocation,
            message,
        }
    }

    fn intent(&self) -> FederationDeliveryIntent<'_> {
        FederationDeliveryIntent {
            parent: &self.parent,
            reconciliation_resources: std::slice::from_ref(&self.reconciliation_resource),
            endpoint: "https://remote.example/a2a",
            agent_id: "responder",
            envelope: &self.envelope,
            invocation: &self.invocation,
            message: &self.message,
        }
    }

    fn remote_receipt(&self) -> FederationImportReceipt {
        let envelope = self.envelope.envelope().unwrap();
        FederationImportReceipt {
            namespace: envelope.agent_resource.namespace().into(),
            tenant: envelope.agent_resource.tenant().into(),
            import_id: envelope.id,
            envelope_digest: self.envelope.digest().unwrap(),
            trust_id: envelope.trust_id,
            trust_revision: envelope.trust_revision,
            issuer_domain: envelope.issuer_domain,
            source: envelope.source,
            target: envelope.target,
            agent_resource: envelope.agent_resource,
            binding_digest: envelope.binding_digest,
            skill: envelope.skill,
            root_id: "remote-root-1".into(),
            expires_at_ms: envelope.expires_at_ms,
            accepted_at_ms: 110,
            execution_started: false,
        }
    }

    fn remote_task(&self) -> Task {
        let mut task = Task::new_at(
            self.invocation.digest().unwrap()[..32].to_owned(),
            "remote",
            "tenant",
            chrono::DateTime::from_timestamp_millis(110).unwrap(),
        );
        let mut message = self.message.clone();
        message.task_id = Some(task.id.clone());
        task.append_history_at(
            message,
            chrono::DateTime::from_timestamp_millis(110).unwrap(),
        )
        .unwrap();
        task
    }

    fn finality_reconciler(&self, key: &[u8]) -> FederationDeliveryFinalityReconciler {
        let verifier: Arc<dyn FederationFinalityVerifier> = Arc::new(
            HmacFederationFinalityVerifier::new_trusted(
                "remote-finality-journal",
                "contracts/remote-finality-v1",
                "remote-finality-v1",
                BTreeMap::from([("finality-key-1".into(), key.to_vec())]),
            )
            .unwrap(),
        );
        let state: Arc<dyn StateStore> = self.state.clone();
        FederationDeliveryFinalityReconciler::new_trusted(
            state,
            self.clock.clone(),
            None,
            BTreeMap::from([(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                verifier,
            )]),
        )
        .unwrap()
    }
}

#[tokio::test]
async fn capability_negotiation_is_durable_and_unavailability_never_imports() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .capability_mode
        .store(UNCERTAIN, Ordering::SeqCst);

    let pending = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        pending.status,
        FederationDeliveryStatus::Registered
    ));
    assert_eq!(fixture.adapter.negotiations.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 0);

    fixture
        .adapter
        .capability_mode
        .store(ACCEPT, Ordering::SeqCst);
    let accepted = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        accepted.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.negotiations.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    let evidence = fixture
        .transport
        .list_evidence("local", "tenant", None, 10)
        .await
        .unwrap();
    let capability = evidence[0].capability.as_ref().unwrap();
    assert_eq!(capability.card_digest, "c".repeat(64));
    assert_eq!(capability.protocol_revision, "acteon-federation-v1");
}

#[tokio::test]
async fn capability_claim_is_visible_without_exposing_the_card() {
    let fixture = Arc::new(Fixture::new(true));
    fixture
        .adapter
        .block_capability
        .store(true, Ordering::SeqCst);
    let delivery = {
        let fixture = fixture.clone();
        tokio::spawn(async move { fixture.transport.submit(fixture.intent()).await })
    };
    fixture.adapter.capability_started.notified().await;
    let evidence = fixture
        .transport
        .list_evidence("local", "tenant", None, 10)
        .await
        .unwrap();
    assert!(matches!(
        evidence[0].status,
        FederationDeliveryEvidenceStatus::CapabilityNegotiating { .. }
    ));
    assert!(evidence[0].capability.is_none());
    fixture.adapter.release_capability.notify_one();
    assert!(matches!(
        delivery.await.unwrap().unwrap().status,
        FederationDeliveryStatus::Accepted { .. }
    ));
}

#[tokio::test]
async fn exact_signed_intent_is_durable_before_one_import_and_one_invocation() {
    let fixture = Fixture::new(true);
    let receipt = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        receipt.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert!(fixture.adapter.saw_persisted_intent.load(Ordering::SeqCst));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);

    let replay = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        replay.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn registration_can_be_separated_from_authorized_delivery() {
    let fixture = Fixture::new(true);
    let registered = fixture.transport.register(fixture.intent()).await.unwrap();
    assert!(matches!(
        registered.status,
        FederationDeliveryStatus::Registered
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 0);

    let delivered = fixture
        .transport
        .deliver_registered(fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        delivered.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconciliation_evidence_is_sanitized_bounded_and_expiry_aware() {
    let fixture = Fixture::new(true);
    let registered = fixture.transport.register(fixture.intent()).await.unwrap();
    let evidence = fixture
        .transport
        .list_evidence("local", "tenant", None, 1)
        .await
        .unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].delivery_id, registered.delivery_id);
    assert_eq!(
        evidence[0].parent_execution_id,
        fixture.parent.execution_id()
    );
    assert_eq!(evidence[0].source.id(), "source-agent");
    assert_eq!(evidence[0].message_id, "message-1");
    assert_eq!(evidence[0].trust_id, "remote-trust-1");
    assert_eq!(evidence[0].key_id, "local-ed25519-1");
    assert_eq!(evidence[0].issuer_epoch, 1);
    assert!(!evidence[0].expired);
    assert!(matches!(
        evidence[0].status,
        FederationDeliveryEvidenceStatus::Registered
    ));
    let exhausted = fixture
        .transport
        .list_evidence("local", "tenant", Some(registered.delivery_id), 1)
        .await
        .unwrap();
    assert_eq!(exhausted.len(), 0);

    fixture
        .clock
        .advance_to(Duration::from_millis(4_000))
        .unwrap();
    assert!(
        fixture
            .transport
            .list_evidence("local", "tenant", None, 1)
            .await
            .unwrap()[0]
            .expired
    );
    assert!(matches!(
        fixture
            .transport
            .list_evidence("local", "tenant", None, 0)
            .await,
        Err(FederationTransportError::Invalid)
    ));
}

#[tokio::test]
async fn concurrent_submission_has_one_network_owner() {
    let fixture = Arc::new(Fixture::new(true));
    fixture.adapter.block_import.store(true, Ordering::SeqCst);
    let owner = {
        let fixture = Arc::clone(&fixture);
        tokio::spawn(async move { fixture.transport.submit(fixture.intent()).await })
    };
    fixture.adapter.import_started.notified().await;

    let observer = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        observer.status,
        FederationDeliveryStatus::ImportUncertain
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    fixture.adapter.block_import.store(false, Ordering::SeqCst);
    fixture.adapter.release_import.notify_one();

    let accepted = owner.await.unwrap().unwrap();
    assert!(matches!(
        accepted.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ambiguous_import_never_invokes_and_requires_explicit_safe_replay() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let first = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        first.status,
        FederationDeliveryStatus::ImportUncertain
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 0);

    fixture.transport.submit(fixture.intent()).await.unwrap();
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    fixture.adapter.import_mode.store(ACCEPT, Ordering::SeqCst);
    let recovered = fixture
        .transport
        .replay_idempotent(fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        recovered.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);

    let unsafe_fixture = Fixture::new(false);
    unsafe_fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    unsafe_fixture
        .transport
        .submit(unsafe_fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        unsafe_fixture
            .transport
            .replay_idempotent(unsafe_fixture.intent())
            .await,
        Err(FederationTransportError::Refused)
    ));
}

#[tokio::test]
async fn operator_can_abandon_ambiguity_without_asserting_remote_finality_or_replay() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let uncertain = fixture.transport.submit(fixture.intent()).await.unwrap();
    let source = fixture.parent.principal().clone();
    let abandonment = FederationDeliveryAbandonment {
        disposition_id: uuid::Uuid::new_v4(),
        operator: PrincipalIdentity::new("incident-operator", PrincipalKind::Human).unwrap(),
        authority: AuthorityStamp {
            incarnation: "scope-incarnation".into(),
            generation: 7,
        },
        reason: "peer owner confirmed local recovery must stop".into(),
        recorded_at_ms: fixture.clock.now().timestamp_millis(),
    };
    let state: Arc<dyn StateStore> = fixture.state.clone();
    let reconciler = FederationDeliveryReconciler::new(state, fixture.clock.clone(), None);
    let wrong_source = PrincipalIdentity::new("other-agent", PrincipalKind::Agent).unwrap();
    assert!(matches!(
        reconciler
            .abandon(
                "local",
                "tenant",
                uncertain.delivery_id,
                &wrong_source,
                abandonment.clone(),
            )
            .await,
        Err(FederationTransportError::Conflict)
    ));
    let evidence = reconciler
        .abandon(
            "local",
            "tenant",
            uncertain.delivery_id,
            &source,
            abandonment.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(
        evidence.status,
        FederationDeliveryEvidenceStatus::Abandoned {
            stage: acteon_executor::federation_transport::FederationDeliveryStage::Import,
            ref abandonment,
        } if abandonment.operator.id() == "incident-operator"
    ));

    let replayed = fixture
        .transport
        .replay_idempotent(fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        replayed.status,
        FederationDeliveryStatus::Abandoned { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 0);

    let repeated = reconciler
        .abandon(
            "local",
            "tenant",
            uncertain.delivery_id,
            &source,
            abandonment,
        )
        .await
        .unwrap();
    assert!(matches!(
        repeated.status,
        FederationDeliveryEvidenceStatus::Abandoned { .. }
    ));
}

#[tokio::test]
async fn invoke_abandonment_retains_and_validates_import_lineage() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .invoke_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let uncertain = fixture.transport.submit(fixture.intent()).await.unwrap();
    let source = fixture.parent.principal().clone();
    let state: Arc<dyn StateStore> = fixture.state.clone();
    let reconciler = FederationDeliveryReconciler::new(state.clone(), fixture.clock.clone(), None);
    let evidence = reconciler
        .abandon(
            "local",
            "tenant",
            uncertain.delivery_id,
            &source,
            FederationDeliveryAbandonment {
                disposition_id: uuid::Uuid::new_v4(),
                operator: PrincipalIdentity::new("incident-operator", PrincipalKind::Human)
                    .unwrap(),
                authority: AuthorityStamp {
                    incarnation: "scope-incarnation".into(),
                    generation: 8,
                },
                reason: "stop recovery after external investigation".into(),
                recorded_at_ms: fixture.clock.now().timestamp_millis(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        evidence.status,
        FederationDeliveryEvidenceStatus::Abandoned {
            stage: acteon_executor::federation_transport::FederationDeliveryStage::Invoke,
            ..
        }
    ));

    let key = StateKey::new(
        "local",
        "tenant",
        KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
        uncertain.delivery_id.to_string(),
    );
    let raw = fixture.state.get(&key).await.unwrap().unwrap();
    let mut corrupted: serde_json::Value = serde_json::from_str(&raw).unwrap();
    corrupted["state"]["receipt"] = serde_json::Value::Null;
    fixture
        .state
        .set(&key, &corrupted.to_string(), None)
        .await
        .unwrap();
    let reader = FederationDeliveryEvidenceReader::new(state, fixture.clock.clone(), None);
    assert!(matches!(
        reader
            .get_evidence("local", "tenant", uncertain.delivery_id)
            .await,
        Err(FederationTransportError::Conflict)
    ));
}

#[tokio::test]
async fn qualified_import_finality_advances_without_reimport_and_keeps_audit_evidence() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let uncertain = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        uncertain.status,
        FederationDeliveryStatus::ImportUncertain
    ));
    let key = [71_u8; 32];
    let reconciler = fixture.finality_reconciler(&key);
    let correlation = reconciler
        .correlation(
            "local",
            "tenant",
            uncertain.delivery_id,
            fixture.parent.principal(),
        )
        .await
        .unwrap();
    let proof = sign_federation_finality_receipt(
        correlation,
        FederationPeerFinality::ImportAccepted {
            receipt: Box::new(fixture.remote_receipt()),
        },
        "remote-finality-v1",
        "finality-key-1",
        &key,
    )
    .unwrap();
    let acceptance = FederationFinalityAcceptance {
        operator: PrincipalIdentity::new("incident-operator", PrincipalKind::Human).unwrap(),
        authority: AuthorityStamp {
            incarnation: "scope-incarnation".into(),
            generation: 11,
        },
        accepted_at_ms: fixture.clock.now().timestamp_millis(),
    };
    let evidence = reconciler
        .reconcile(
            "local",
            "tenant",
            uncertain.delivery_id,
            fixture.parent.principal(),
            &proof,
            acceptance.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(
        evidence.status,
        FederationDeliveryEvidenceStatus::Imported
    ));
    assert_eq!(evidence.reconciliations.len(), 1);
    assert!(evidence.reconciliations[0].accepted);
    assert_eq!(evidence.reconciliations[0].acceptance, acceptance);

    // Exact proof replay repairs a lost local acknowledgment without changing
    // peer state or issuing network traffic.
    reconciler
        .reconcile(
            "local",
            "tenant",
            uncertain.delivery_id,
            fixture.parent.principal(),
            &proof,
            FederationFinalityAcceptance {
                operator: PrincipalIdentity::new("recovery-operator", PrincipalKind::Human)
                    .unwrap(),
                authority: AuthorityStamp {
                    incarnation: "scope-incarnation".into(),
                    generation: 12,
                },
                accepted_at_ms: fixture.clock.now().timestamp_millis(),
            },
        )
        .await
        .unwrap();
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);

    fixture.adapter.import_mode.store(ACCEPT, Ordering::SeqCst);
    let delivered = fixture
        .transport
        .deliver_registered(fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        delivered.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
    let retained =
        FederationDeliveryEvidenceReader::new(fixture.state.clone(), fixture.clock.clone(), None)
            .get_evidence("local", "tenant", uncertain.delivery_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(retained.reconciliations.len(), 1);
    assert_eq!(
        retained.reconciliations[0].acceptance.operator.id(),
        "incident-operator"
    );
}

#[tokio::test]
async fn qualified_invocation_finality_closes_ambiguity_without_a_second_send() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .invoke_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let uncertain = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        uncertain.status,
        FederationDeliveryStatus::InvokeUncertain { .. }
    ));
    let key = [72_u8; 32];
    let reconciler = fixture.finality_reconciler(&key);
    let correlation = reconciler
        .correlation(
            "local",
            "tenant",
            uncertain.delivery_id,
            fixture.parent.principal(),
        )
        .await
        .unwrap();
    let proof = sign_federation_finality_receipt(
        correlation,
        FederationPeerFinality::InvocationAccepted {
            receipt: Box::new(fixture.remote_receipt()),
            task: Box::new(fixture.remote_task()),
        },
        "remote-finality-v1",
        "finality-key-1",
        &key,
    )
    .unwrap();
    let evidence = reconciler
        .reconcile(
            "local",
            "tenant",
            uncertain.delivery_id,
            fixture.parent.principal(),
            &proof,
            FederationFinalityAcceptance {
                operator: PrincipalIdentity::new("incident-operator", PrincipalKind::Human)
                    .unwrap(),
                authority: AuthorityStamp {
                    incarnation: "scope-incarnation".into(),
                    generation: 12,
                },
                accepted_at_ms: fixture.clock.now().timestamp_millis(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        evidence.status,
        FederationDeliveryEvidenceStatus::Accepted
    ));
    assert_eq!(evidence.reconciliations.len(), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
    let observed = fixture.transport.observe(fixture.intent()).await.unwrap();
    assert!(matches!(
        observed.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unqualified_or_mismatched_finality_cannot_change_delivery_state() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let uncertain = fixture.transport.submit(fixture.intent()).await.unwrap();
    let trusted_key = [73_u8; 32];
    let reconciler = fixture.finality_reconciler(&trusted_key);
    let correlation = reconciler
        .correlation(
            "local",
            "tenant",
            uncertain.delivery_id,
            fixture.parent.principal(),
        )
        .await
        .unwrap();
    let proof = sign_federation_finality_receipt(
        correlation,
        FederationPeerFinality::ImportRejected {
            code: "peer_did_not_import".into(),
        },
        "remote-finality-v1",
        "finality-key-1",
        &[74_u8; 32],
    )
    .unwrap();
    assert!(matches!(
        reconciler
            .reconcile(
                "local",
                "tenant",
                uncertain.delivery_id,
                fixture.parent.principal(),
                &proof,
                FederationFinalityAcceptance {
                    operator: PrincipalIdentity::new("incident-operator", PrincipalKind::Human)
                        .unwrap(),
                    authority: AuthorityStamp {
                        incarnation: "scope-incarnation".into(),
                        generation: 13,
                    },
                    accepted_at_ms: fixture.clock.now().timestamp_millis(),
                },
            )
            .await,
        Err(FederationTransportError::Refused)
    ));
    let observed = fixture.transport.observe(fixture.intent()).await.unwrap();
    assert!(matches!(
        observed.status,
        FederationDeliveryStatus::ImportUncertain
    ));
}

#[tokio::test]
async fn known_import_survives_invocation_response_loss_without_reimport() {
    let fixture = Fixture::new(true);
    fixture
        .adapter
        .invoke_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let first = fixture.transport.submit(fixture.intent()).await.unwrap();
    assert!(matches!(
        first.status,
        FederationDeliveryStatus::InvokeUncertain { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);

    fixture.transport.submit(fixture.intent()).await.unwrap();
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
    fixture.adapter.invoke_mode.store(ACCEPT, Ordering::SeqCst);
    let recovered = fixture
        .transport
        .replay_idempotent(fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        recovered.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn retained_message_substitution_is_detected_before_observation_or_delivery() {
    let fixture = Fixture::new(true);
    let receipt = fixture.transport.submit(fixture.intent()).await.unwrap();
    let key = StateKey::new(
        "local",
        "tenant",
        KeyKind::Custom(FEDERATION_DELIVERY_KIND.into()),
        receipt.delivery_id.to_string(),
    );
    let raw = fixture.state.get(&key).await.unwrap().unwrap();
    let mut corrupted: serde_json::Value = serde_json::from_str(&raw).unwrap();
    corrupted["message"]["parts"][0]["text"] = serde_json::json!("substituted");
    fixture
        .state
        .set(&key, &corrupted.to_string(), None)
        .await
        .unwrap();
    assert!(matches!(
        fixture.transport.observe(fixture.intent()).await,
        Err(FederationTransportError::Conflict)
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exact_intent_rejects_message_substitution_before_persistence() {
    let mut fixture = Fixture::new(true);
    let mut changed = serde_json::to_value(&fixture.message).unwrap();
    changed["parts"][0]["text"] = serde_json::json!("different");
    fixture.message = serde_json::from_value(changed).unwrap();
    assert!(matches!(
        fixture.transport.submit(fixture.intent()).await,
        Err(FederationTransportError::Conflict)
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.clock.now().timestamp_millis(), 100);
}

#[tokio::test]
async fn expired_claim_requires_explicit_idempotent_recovery() {
    let fixture = Arc::new(Fixture::new(true));
    fixture.adapter.block_import.store(true, Ordering::SeqCst);
    let running = {
        let fixture = Arc::clone(&fixture);
        tokio::spawn(async move { fixture.transport.submit(fixture.intent()).await })
    };
    fixture.adapter.import_started.notified().await;

    let in_flight = fixture.transport.observe(fixture.intent()).await.unwrap();
    assert!(matches!(
        in_flight.status,
        FederationDeliveryStatus::ImportUncertain
    ));
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());

    fixture.transport.submit(fixture.intent()).await.unwrap();
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    fixture
        .clock
        .advance_to(Duration::from_millis(1_001))
        .unwrap();
    fixture.adapter.block_import.store(false, Ordering::SeqCst);
    let recovered = fixture
        .transport
        .replay_idempotent(fixture.intent())
        .await
        .unwrap();
    assert!(matches!(
        recovered.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn signed_expiry_blocks_network_but_preserves_terminal_observation() {
    let expired = Fixture::new(true);
    expired
        .clock
        .advance_to(Duration::from_millis(5_000))
        .unwrap();
    let expiry = expired.transport.submit(expired.intent()).await.unwrap();
    assert!(matches!(
        expiry.status,
        FederationDeliveryStatus::Rejected {
            stage: acteon_executor::federation_transport::FederationDeliveryStage::Import,
            ref code,
        } if code == "signed_request_expired"
    ));
    assert_eq!(expired.adapter.imports.load(Ordering::SeqCst), 0);
    assert_eq!(expired.adapter.invokes.load(Ordering::SeqCst), 0);

    let accepted = Fixture::new(true);
    accepted.transport.submit(accepted.intent()).await.unwrap();
    accepted
        .clock
        .advance_to(Duration::from_millis(5_000))
        .unwrap();
    let observed = accepted.transport.observe(accepted.intent()).await.unwrap();
    assert!(matches!(
        observed.status,
        FederationDeliveryStatus::Accepted { .. }
    ));
    assert_eq!(accepted.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(accepted.adapter.invokes.load(Ordering::SeqCst), 1);
}
