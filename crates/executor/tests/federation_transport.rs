use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_core::{
    ExecutionContextReference, FederationImportReceipt, PrincipalIdentity, PrincipalKind,
    ResourceKind, ResourceRef, Task, TaskMessage, TaskRole,
};
use acteon_executor::federation_transport::{
    DurableFederationTransport, FEDERATION_DELIVERY_KIND, FederationDeliveryIntent,
    FederationDeliveryStatus, FederationImportDisposition, FederationImportRequest,
    FederationInvokeDisposition, FederationInvokeRequest, FederationTransportAdapter,
    FederationTransportDependencies, FederationTransportError,
};
use acteon_governance::{
    RootBudgetLimits,
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
    import_mode: AtomicU8,
    invoke_mode: AtomicU8,
    imports: AtomicUsize,
    invokes: AtomicUsize,
    saw_persisted_intent: AtomicBool,
    block_import: AtomicBool,
    import_started: Notify,
    release_import: Notify,
}

impl Adapter {
    fn new(state: Arc<dyn StateStore>, replay_safe: bool) -> Self {
        Self {
            state,
            replay_safe,
            import_mode: AtomicU8::new(ACCEPT),
            invoke_mode: AtomicU8::new(ACCEPT),
            imports: AtomicUsize::new(0),
            invokes: AtomicUsize::new(0),
            saw_persisted_intent: AtomicBool::new(false),
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
            envelope,
            invocation,
            message,
        }
    }

    fn intent(&self) -> FederationDeliveryIntent<'_> {
        FederationDeliveryIntent {
            parent: &self.parent,
            endpoint: "https://remote.example/a2a",
            agent_id: "responder",
            envelope: &self.envelope,
            invocation: &self.invocation,
            message: &self.message,
        }
    }
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
