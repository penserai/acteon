use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_core::{
    FederationImportReceipt, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef, Task,
    TaskMessage, TaskRole,
};
use acteon_executor::{
    federation_export::{
        FederationExportDependencies, FederationExportRequest, GovernedFederationExporter,
        QualifiedFederationExport,
    },
    federation_transport::{
        DurableFederationTransport, FederationDeliveryAbandonment, FederationDeliveryReconciler,
        FederationDeliveryStatus, FederationImportDisposition, FederationImportRequest,
        FederationInvokeDisposition, FederationInvokeRequest, FederationTransportAdapter,
        FederationTransportDependencies, FederationTransportError,
    },
};
use acteon_governance::{
    AttemptStatus, AuthorityChange, AuthorityCoordinator, AuthorityStamp, CoordinatorLimits,
    RootBudgetLimits, ScopePurpose,
    context::{
        AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
        RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
    },
    credential::{CredentialAuthority, CredentialReference},
    federation::FederationEnvelopeSigner,
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference},
};
use acteon_state::StateStore;
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use async_trait::async_trait;

const ACCEPT: u8 = 0;
const UNCERTAIN: u8 = 1;

fn principal(id: &str, kind: PrincipalKind) -> PrincipalIdentity {
    PrincipalIdentity::new(id, kind).unwrap()
}

fn local_export_effect() -> AcceptedEffect {
    AcceptedEffect {
        operation: "federation.export".into(),
        resources: vec![
            ResourceRef::new(
                ResourceKind::ExternalService,
                "local",
                "tenant",
                "remote.example",
            )
            .unwrap(),
        ],
    }
}

fn root_limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 6,
        max_concurrent: 1,
        deadline_ms: 5_000,
    }
}

fn remote_limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 2,
        max_concurrent: 1,
        deadline_ms: 4_000,
    }
}

struct Adapter {
    import_mode: AtomicU8,
    imports: AtomicUsize,
    invokes: AtomicUsize,
}

impl Adapter {
    fn new() -> Self {
        Self {
            import_mode: AtomicU8::new(ACCEPT),
            imports: AtomicUsize::new(0),
            invokes: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl FederationTransportAdapter for Adapter {
    fn revision(&self) -> &'static str {
        "federation-export-test-v1"
    }

    fn binding_digest(&self) -> &'static str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn exact_replay_idempotent(&self) -> bool {
        true
    }

    async fn import(
        &self,
        request: FederationImportRequest<'_>,
    ) -> Result<FederationImportDisposition, FederationTransportError> {
        self.imports.fetch_add(1, Ordering::SeqCst);
        if self.import_mode.load(Ordering::SeqCst) == UNCERTAIN {
            return Ok(FederationImportDisposition::Uncertain);
        }
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
                root_id: "remote-export-root".into(),
                expires_at_ms: envelope.expires_at_ms,
                accepted_at_ms: 110,
                execution_started: false,
            },
        )))
    }

    async fn invoke(
        &self,
        request: FederationInvokeRequest<'_>,
    ) -> Result<FederationInvokeDisposition, FederationTransportError> {
        self.invokes.fetch_add(1, Ordering::SeqCst);
        let mut task = Task::new_at(
            "remote-export-task",
            request.namespace,
            request.tenant,
            chrono::DateTime::from_timestamp_millis(110).unwrap(),
        );
        let mut message = request.message.clone();
        message.task_id = Some(task.id.clone());
        task.append_history_at(
            message,
            chrono::DateTime::from_timestamp_millis(110).unwrap(),
        )
        .map_err(|_| FederationTransportError::Unavailable)?;
        Ok(FederationInvokeDisposition::Accepted(Box::new(task)))
    }
}

struct Fixture {
    state: Arc<dyn StateStore>,
    clock: Arc<ManualClock>,
    coordinator: AuthorityCoordinator,
    adapter: Arc<Adapter>,
    exporter: GovernedFederationExporter,
    context: VerifiedExecutionContext,
    permits: Vec<PermitReference>,
    permit: ExecutionPermit,
    message: TaskMessage,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    async fn new() -> Self {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let coordinator = AuthorityCoordinator::initialize(
            state.clone(),
            "local",
            "tenant",
            CoordinatorLimits::default(),
        )
        .await
        .unwrap();
        coordinator
            .reserve_scope(ScopePurpose::Execution)
            .await
            .unwrap();
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp_millis(100).unwrap(),
        ));
        let source = principal("source-agent", PrincipalKind::Agent);
        let operator = principal("operator", PrincipalKind::Human);
        let permit = ExecutionPermit {
            id: "source-export".into(),
            revision: 1,
            subject: source.clone(),
            effects: vec![local_export_effect()],
            valid_from_ms: 0,
            limits: root_limits(),
        };
        let permit_ceiling = PermitIssuanceCeiling {
            issuer: operator,
            subjects: vec![source.clone()],
            effects: vec![local_export_effect()],
            valid_from_ms: 0,
            limits: root_limits(),
        };
        coordinator
            .publish_permit(
                "publish-source-export",
                permit.clone(),
                0,
                &permit_ceiling,
                &coordinator.snapshot().await.unwrap().stamp(),
                "allow remote service",
                100,
            )
            .await
            .unwrap();
        coordinator
            .publish_credential(
                "publish-source-credential",
                CredentialAuthority {
                    ceiling: ExecutionPermit {
                        id: "source-key".into(),
                        ..permit.clone()
                    },
                    auth_method: "api_key".into(),
                    execution_enabled: true,
                },
                0,
                &permit_ceiling,
                &coordinator.snapshot().await.unwrap().stamp(),
                "authenticate source",
                100,
            )
            .await
            .unwrap();
        let permits = vec![PermitReference {
            id: permit.id.clone(),
            accepted_revision: 1,
        }];
        let contexts = TrustedContextStore::new(
            state.clone(),
            coordinator.clone(),
            "local.example".into(),
            "context-key".into(),
            vec![ContextSigningKey::new("context-key".into(), vec![7; 32]).unwrap()],
        )
        .unwrap();
        let context = contexts
            .capture_credentialed_root(
                RootContextAdmission {
                    handle: ExecutionContextHandle::new(),
                    binding: ContextBinding {
                        execution_id: uuid::Uuid::new_v4(),
                        principal: source,
                        request_digest: "c".repeat(64),
                    },
                    credential_id: "source-key".into(),
                    auth_method: "api_key".into(),
                    accepted_ceiling_revision: "host".into(),
                    accepted_effects: vec![local_export_effect()],
                    deadline_ms: 5_000,
                    evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
                },
                &permits,
                CredentialReference {
                    id: "source-key".into(),
                    accepted_revision: 1,
                },
                root_limits(),
                clock.as_ref(),
            )
            .await
            .unwrap();
        let adapter = Arc::new(Adapter::new());
        let transport = Arc::new(
            DurableFederationTransport::new_trusted(
                FederationTransportDependencies {
                    state: state.clone(),
                    clock: clock.clone(),
                    encryptor: None,
                },
                adapter.clone(),
                Duration::from_secs(1),
            )
            .unwrap(),
        );
        let signer_secret = [31; 32];
        let verifying_key =
            FederationEnvelopeSigner::from_secret(signer_secret).verifying_key_hex();
        let remote_agent =
            ResourceRef::new(ResourceKind::Agent, "remote", "tenant", "responder").unwrap();
        let remote_provider =
            ResourceRef::new(ResourceKind::Provider, "remote", "tenant", "incident-api").unwrap();
        let ingress = AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: vec![remote_agent.clone(), remote_provider.clone()],
        };
        let profile = QualifiedFederationExport::new_trusted(
            "https://remote.example",
            "local.example",
            "remote.example",
            "remote-trust-1",
            1,
            "local-ed25519-1",
            verifying_key,
            1,
            principal("remote-agent", PrincipalKind::Agent),
            remote_agent,
            "b".repeat(64),
            "respond",
            ingress,
            vec![AcceptedEffect {
                operation: "incident.inspect".into(),
                resources: vec![remote_provider],
            }],
            RootBudgetLimits {
                max_units: 3,
                max_concurrent: 1,
                deadline_ms: 4_900,
            },
            2,
            4_900,
            local_export_effect(),
        )
        .unwrap();
        let exporter = GovernedFederationExporter::new_trusted(
            FederationExportDependencies {
                coordinator: coordinator.clone(),
                clock: clock.clone(),
                transport,
            },
            signer_secret,
            profile,
        )
        .unwrap();
        Self {
            state,
            clock,
            coordinator,
            adapter,
            exporter,
            context,
            permits,
            permit,
            message: TaskMessage::text("export-message-1", TaskRole::User, "inspect incident"),
        }
    }

    fn request(&self) -> FederationExportRequest<'_> {
        FederationExportRequest {
            context: &self.context,
            permits: &self.permits,
            export_id: "export-source-1",
            invocation_nonce: "invocation-source-1",
            message: &self.message,
            issued_at_ms: 100,
            expires_at_ms: 4_500,
            limits: remote_limits(),
            max_depth: 1,
        }
    }

    async fn revoke(&self) {
        self.coordinator
            .change(
                "revoke-source-export",
                AuthorityChange::RevokePermit {
                    permit_id: self.permit.id.clone(),
                    expected_revision: 1,
                },
                "operator",
                "close export",
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn exact_export_reserves_local_budget_before_one_remote_delivery() {
    let fixture = Fixture::new().await;
    let receipt = fixture.exporter.submit(fixture.request()).await.unwrap();
    assert_eq!(receipt.attempt_status, AttemptStatus::Settled);
    let FederationDeliveryStatus::Accepted {
        receipt: remote, ..
    } = &receipt.delivery.status
    else {
        panic!("expected accepted federated task");
    };
    assert_eq!(remote.issuer_domain, "local.example");
    assert_eq!(remote.agent_resource.namespace(), "remote");
    assert_eq!(remote.source, *fixture.context.principal());
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
    let snapshot = fixture.coordinator.snapshot().await.unwrap();
    let root = &snapshot.roots[&fixture.context.execution_id().to_string()];
    assert_eq!(root.spent_units, 2);
    assert_eq!(root.active_attempts, 0);
    assert_eq!(
        snapshot.starts[&receipt.attempt_id].resources,
        local_export_effect().resources.into_iter().collect()
    );

    let replay = fixture.exporter.submit(fixture.request()).await.unwrap();
    assert_eq!(replay.attempt_id, receipt.attempt_id);
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.coordinator.snapshot().await.unwrap().roots
            [&fixture.context.execution_id().to_string()]
            .spent_units,
        2
    );
}

#[tokio::test]
async fn ambiguous_export_retains_capacity_and_revocation_blocks_explicit_replay() {
    let fixture = Fixture::new().await;
    fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let first = fixture.exporter.submit(fixture.request()).await.unwrap();
    assert_eq!(first.attempt_status, AttemptStatus::Uncertain);
    assert!(matches!(
        first.delivery.status,
        FederationDeliveryStatus::ImportUncertain
    ));
    let snapshot = fixture.coordinator.snapshot().await.unwrap();
    let root = &snapshot.roots[&fixture.context.execution_id().to_string()];
    assert_eq!((root.spent_units, root.active_attempts), (2, 1));

    fixture.revoke().await;
    fixture.adapter.import_mode.store(ACCEPT, Ordering::SeqCst);
    assert!(
        fixture
            .exporter
            .replay_idempotent(fixture.request())
            .await
            .is_err()
    );
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
    let observed = fixture.exporter.observe(fixture.request()).await.unwrap();
    assert_eq!(observed.attempt_status, AttemptStatus::Uncertain);
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn durable_operator_abandonment_releases_retained_export_capacity() {
    let fixture = Fixture::new().await;
    fixture
        .adapter
        .import_mode
        .store(UNCERTAIN, Ordering::SeqCst);
    let first = fixture.exporter.submit(fixture.request()).await.unwrap();
    assert_eq!(first.attempt_status, AttemptStatus::Uncertain);
    let snapshot = fixture.coordinator.snapshot().await.unwrap();
    let attempt = snapshot.starts[&first.attempt_id].clone();
    let reconciler =
        FederationDeliveryReconciler::new(fixture.state.clone(), fixture.clock.clone(), None);
    let abandoned = reconciler
        .abandon(
            "local",
            "tenant",
            first.delivery.delivery_id,
            fixture.context.principal(),
            FederationDeliveryAbandonment {
                disposition_id: uuid::Uuid::new_v4(),
                operator: principal("incident-operator", PrincipalKind::Human),
                authority: AuthorityStamp {
                    incarnation: snapshot.incarnation,
                    generation: snapshot.generation,
                },
                reason: "peer finality unavailable; stop local recovery".into(),
                recorded_at_ms: 100,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        abandoned.status,
        acteon_executor::federation_transport::FederationDeliveryEvidenceStatus::Abandoned { .. }
    ));
    fixture
        .coordinator
        .settle(&first.attempt_id, &attempt.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let settled = fixture.coordinator.snapshot().await.unwrap();
    assert_eq!(
        settled.starts[&first.attempt_id].status,
        AttemptStatus::Settled
    );
    assert_eq!(
        settled.roots[&fixture.context.execution_id().to_string()].active_attempts,
        0
    );
    let replay = fixture.exporter.observe(fixture.request()).await.unwrap();
    assert_eq!(replay.attempt_status, AttemptStatus::Settled);
    assert!(matches!(
        replay.delivery.status,
        FederationDeliveryStatus::Abandoned { .. }
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn revocation_after_reservation_blocks_a_not_yet_delivered_export() {
    let fixture = Fixture::new().await;
    let authorized = fixture
        .exporter
        .authorize(&fixture.request())
        .await
        .unwrap();
    assert_eq!(authorized.attempt_status, AttemptStatus::InFlight);
    assert!(matches!(
        authorized.delivery.status,
        FederationDeliveryStatus::Registered
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 0);

    fixture.revoke().await;
    let denied = fixture
        .exporter
        .deliver_authorized(fixture.request())
        .await
        .unwrap();
    assert_eq!(denied.attempt_status, AttemptStatus::Settled);
    assert!(matches!(
        denied.delivery.status,
        FederationDeliveryStatus::Rejected { ref code, .. }
            if code == "source_authority_denied"
    ));
    assert_eq!(fixture.adapter.imports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.adapter.invokes.load(Ordering::SeqCst), 0);
    let snapshot = fixture.coordinator.snapshot().await.unwrap();
    let root = &snapshot.roots[&fixture.context.execution_id().to_string()];
    assert_eq!((root.spent_units, root.active_attempts), (2, 0));
}
