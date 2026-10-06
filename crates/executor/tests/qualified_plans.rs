use acteon_core::{
    Action, ActionOutcome, ChainConfig, ChainStepConfig, PrincipalIdentity, PrincipalKind,
    ProviderResponse, ResourceKind, ResourceRef,
};
use acteon_executor::{
    ExecutorConfig, GovernedProviderMediator, ProviderExecutionAdmission,
    ProviderExecutionMediator, ProviderInvocation, ProviderInvocationOrigin,
    catalog::QualifiedProviderCatalog,
    governed::{BoundProvider, GovernedProviderExecutor, GovernedProviderStatus},
    plan::{PlanCallSite, PlanChildCapture, PlanError, PlanProviderAdmission, QualifiedChainPlan},
};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, CoordinatorLimits, RootBudgetLimits, ScopePurpose,
    context::{
        ContextBinding, ContextSigningKey, ExecutionContextHandle, RootContextAdmission,
        TrustedContextStore,
    },
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference, permit_revision_tag},
};
use acteon_provider::{DynProvider, ProviderError};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Counter {
    calls: AtomicUsize,
}
#[async_trait]
impl DynProvider for Counter {
    fn name(&self) -> &'static str {
        "incident"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderResponse::success(action.payload.clone()))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
fn catalog(provider: Arc<dyn DynProvider>, revision: &str) -> QualifiedProviderCatalog {
    QualifiedProviderCatalog::new_trusted(vec![
        BoundProvider::new_trusted(
            provider,
            &ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "incident-http").unwrap(),
            "execute",
            revision,
            vec![],
        )
        .unwrap(),
    ])
    .unwrap()
}
fn definitions() -> BTreeMap<String, ChainConfig> {
    BTreeMap::from([(
        "diagnose".into(),
        ChainConfig::new("diagnose")
            .with_step(ChainStepConfig::new(
                "metrics",
                "incident",
                "execute",
                json!({"source":"metrics"}),
            ))
            .with_step(ChainStepConfig::new(
                "logs",
                "incident",
                "execute",
                json!({"source":"logs"}),
            )),
    )])
}
fn origin() -> Action {
    Action::new(
        "city",
        "tenant",
        "incident",
        "execute",
        json!({"incident":42}),
    )
}
fn site(name: &str) -> PlanCallSite {
    PlanCallSite::Step {
        chain: "diagnose".into(),
        path: vec![name.into()],
    }
}
fn actor() -> PrincipalIdentity {
    PrincipalIdentity::new("worker", PrincipalKind::Agent).unwrap()
}
fn limits(units: u64) -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: units,
        max_concurrent: 1,
        deadline_ms: 1000,
    }
}
fn refs() -> Vec<PermitReference> {
    vec![PermitReference {
        id: "work".into(),
        accepted_revision: 1,
    }]
}

#[test]
fn entire_definition_input_and_route_revision_are_bound() {
    let selected: Arc<dyn DynProvider> = Arc::new(Counter {
        calls: AtomicUsize::new(0),
    });
    let p = Arc::new(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &definitions(),
            catalog(selected.clone(), "v1"),
        )
        .unwrap(),
    );
    let input = origin();
    let bound = p.bind_input(&input).unwrap();
    let mut different = input.clone();
    different.payload = json!({"incident":43});
    assert_ne!(
        bound.request_digest(),
        p.bind_input(&different).unwrap().request_digest()
    );
    let mut changed = definitions();
    changed.get_mut("diagnose").unwrap().steps[1].payload_template = json!({"source":"traces"});
    let changed = Arc::new(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &changed,
            catalog(selected.clone(), "v1"),
        )
        .unwrap(),
    );
    assert_ne!(p.definition_digest(), changed.definition_digest());
    let replaced = QualifiedChainPlan::new_trusted(
        "city",
        "tenant",
        "diagnose",
        &definitions(),
        catalog(selected, "v2"),
    )
    .unwrap();
    assert_ne!(p.definition_digest(), replaced.definition_digest());
    assert_eq!(p.required_effects().len(), 2); // One chain.start plus one deduplicated route.
    different.tenant = "foreign".into();
    assert!(p.bind_input(&different).is_err());
}

#[test]
fn unqualified_cancellation_unreachable_routes_and_subchain_cycles_are_refused() {
    let selected: Arc<dyn DynProvider> = Arc::new(Counter {
        calls: AtomicUsize::new(0),
    });
    let mut changed = definitions();
    changed.get_mut("diagnose").unwrap().steps[0].default_next = Some("finish".into());
    changed
        .get_mut("diagnose")
        .unwrap()
        .steps
        .push(ChainStepConfig::new(
            "finish",
            "incident",
            "execute",
            json!({}),
        ));
    assert_eq!(changed["diagnose"].validate(), Vec::<String>::new());
    changed.get_mut("diagnose").unwrap().steps[1].provider = "undeclared".into();
    assert!(matches!(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &changed,
            catalog(selected.clone(), "v1")
        ),
        Err(PlanError::Unqualified)
    ));
    let mut changed = definitions();
    changed.get_mut("diagnose").unwrap().on_cancel = Some(acteon_core::ChainNotificationTarget {
        provider: "undeclared".into(),
        action_type: "execute".into(),
    });
    assert!(matches!(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &changed,
            catalog(selected.clone(), "v1")
        ),
        Err(PlanError::Unqualified)
    ));
    let changed = BTreeMap::from([(
        "diagnose".into(),
        ChainConfig::new("diagnose").with_step(ChainStepConfig::new_sub_chain("loop", "diagnose")),
    )]);
    assert!(matches!(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &changed,
            catalog(selected, "v1")
        ),
        Err(PlanError::Invalid)
    ));
}

#[tokio::test]
async fn qualified_children_execute_under_one_root_and_chain_closure_blocks_next_effect() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    Box::pin(qualified_handoff_contract(state.clone(), state, false)).await;
}

#[allow(clippy::too_many_lines)] // Ordered admission, recovery and tamper contract.
async fn qualified_handoff_contract(
    state: Arc<dyn StateStore>,
    restart: Arc<dyn StateStore>,
    encrypted: bool,
) {
    use acteon_executor::plan::handoff::{PlanHandoffStore, ReservePlanCall};
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let c = AuthorityCoordinator::initialize(
        state.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    c.reserve_scope(ScopePurpose::Execution).await.unwrap();
    let provider = Arc::new(Counter {
        calls: AtomicUsize::new(0),
    });
    let selected: Arc<dyn DynProvider> = provider.clone();
    let catalog = catalog(selected.clone(), "v1");
    let plan = Arc::new(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &definitions(),
            catalog.clone(),
        )
        .unwrap(),
    );
    let bound = plan.bind_input(&origin()).unwrap();
    c.publish_permit(
        "issue",
        ExecutionPermit {
            id: "work".into(),
            revision: 1,
            subject: actor(),
            effects: plan.required_effects().to_vec(),
            valid_from_ms: 0,
            limits: limits(10),
        },
        0,
        &PermitIssuanceCeiling {
            issuer: actor(),
            subjects: vec![actor()],
            effects: plan.required_effects().to_vec(),
            valid_from_ms: 0,
            limits: limits(10),
        },
        &c.snapshot().await.unwrap().stamp(),
        "reviewed",
        100,
    )
    .await
    .unwrap();
    c.publish_credential(
        "credential-issue",
        acteon_governance::credential::CredentialAuthority {
            ceiling: ExecutionPermit {
                id: "private-key".into(),
                revision: 1,
                subject: actor(),
                effects: plan.required_effects().to_vec(),
                valid_from_ms: 0,
                limits: limits(10),
            },
            auth_method: "api_key".into(),
            execution_enabled: true,
        },
        0,
        &PermitIssuanceCeiling {
            issuer: actor(),
            subjects: vec![actor()],
            effects: plan.required_effects().to_vec(),
            valid_from_ms: 0,
            limits: limits(10),
        },
        &c.snapshot().await.unwrap().stamp(),
        "reviewed private credential",
        100,
    )
    .await
    .unwrap();
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp_millis(100).unwrap(),
    ));
    let contexts = Arc::new(
        TrustedContextStore::new(
            state.clone(),
            c.clone(),
            "plans".into(),
            "key".into(),
            vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
        )
        .unwrap(),
    );
    let root = contexts
        .capture_credentialed_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor(),
                    request_digest: bound.request_digest().into(),
                },
                credential_id: "private-key".into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: permit_revision_tag(&refs()).unwrap(),
                accepted_effects: plan.required_effects().to_vec(),
                deadline_ms: 1000,
                evaluated_authority: c.snapshot().await.unwrap().stamp(),
            },
            &refs(),
            acteon_governance::credential::CredentialReference {
                id: "private-key".into(),
                accepted_revision: 1,
            },
            limits(2),
            clock.as_ref(),
        )
        .await
        .unwrap();
    // Simulate the handoff being committed but its acknowledgement lost. A
    // replacement worker reconstructs the accepted definitions and provenance.
    let faults = Arc::new(FaultStore::new(state.clone()));
    let encryptor = Arc::new(acteon_crypto::PayloadEncryptor::new(
        acteon_crypto::parse_master_key(&"02".repeat(32)).unwrap(),
    ));
    let configure = |store: PlanHandoffStore| {
        if encrypted {
            store.with_encryptor(encryptor.clone())
        } else {
            store
        }
    };
    let handoffs = configure(PlanHandoffStore::new(faults.clone(), "city", "tenant").unwrap());
    let job_id = uuid::Uuid::new_v4();
    faults
        .fail_next(
            KeyKind::Custom("governed_plan_job".into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        handoffs
            .persist(job_id, &bound, &origin(), &root, &refs())
            .await
            .is_err()
    );
    let peer = configure(PlanHandoffStore::new(restart, "city", "tenant").unwrap());
    let recovered = peer
        .recover(job_id, catalog.clone(), &contexts)
        .await
        .unwrap();
    assert_eq!(
        recovered.root().reference().unwrap(),
        root.reference().unwrap()
    );
    assert_eq!(recovered.permits(), refs());
    handoffs
        .persist(job_id, &bound, &origin(), &root, &refs())
        .await
        .unwrap();
    assert!(
        peer.recover(
            job_id,
            self::catalog(selected.clone(), "changed"),
            &contexts
        )
        .await
        .is_err()
    );
    assert!(
        PlanHandoffStore::new(state.clone(), "city", "other")
            .unwrap()
            .recover(job_id, catalog.clone(), &contexts)
            .await
            .is_err()
    );
    let pinned_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom("governed_plan_job".into()),
        job_id.to_string(),
    );
    let original_pin = state.get(&pinned_key).await.unwrap().unwrap();
    if encrypted {
        assert!(original_pin.starts_with("ENC["));
        assert!(!original_pin.contains("incident"));
        assert!(
            PlanHandoffStore::new(state.clone(), "city", "tenant")
                .unwrap()
                .recover(job_id, catalog.clone(), &contexts)
                .await
                .is_err()
        );
    }
    let plaintext = encryptor.decrypt_str(&original_pin).unwrap();
    let mut corrupt: serde_json::Value = serde_json::from_str(&plaintext).unwrap();
    let field = corrupt["definitions"]["diagnose"]["steps"][0]
        .as_object_mut()
        .unwrap();
    field.insert("provider".into(), json!("undeclared"));
    state
        .set(&pinned_key, &serde_json::to_string(&corrupt).unwrap(), None)
        .await
        .unwrap();
    assert!(
        peer.recover(job_id, catalog.clone(), &contexts)
            .await
            .is_err()
    );
    assert!(
        handoffs
            .persist(job_id, &bound, &origin(), &root, &refs())
            .await
            .is_err()
    );
    state.set(&pinned_key, &original_pin, None).await.unwrap();
    // Continue using only the binding reconstructed after handoff.
    let bound = recovered.invocation();
    let mut different_input = origin();
    different_input.payload = json!({"incident":999});
    assert!(
        plan.bind_input(&different_input)
            .unwrap()
            .verify_root(&root)
            .is_err()
    );
    let action = Action::new(
        "city",
        "tenant",
        "incident",
        "execute",
        json!({"source":"metrics"}),
    );
    let logical_attempt = uuid::Uuid::new_v4();
    let call_site = site("metrics");
    let chain_path = vec!["diagnose".into()];
    let request = || ReservePlanCall {
        logical_attempt,
        parent: &root,
        call_site: &call_site,
        chain_path: &chain_path,
        action: &action,
        selected: &selected,
        limits: limits(1),
    };
    faults
        .fail_next(
            KeyKind::Custom("governed_plan_call".into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        handoffs
            .reserve_provider_call(&recovered, request())
            .await
            .is_err()
    );
    let (a, b) = tokio::join!(
        peer.reserve_provider_call(&recovered, request()),
        handoffs.reserve_provider_call(&recovered, request())
    );
    let reserved = a.unwrap();
    let other = b.unwrap();
    assert_eq!(reserved.handle(), other.handle());
    assert_eq!(reserved.execution_id(), other.execution_id());
    assert_eq!(reserved.admission_key(), other.admission_key());
    let mut changed_action = action.clone();
    changed_action.payload = json!({"source":"different"});
    assert!(
        peer.reserve_provider_call(
            &recovered,
            ReservePlanCall {
                action: &changed_action,
                ..request()
            }
        )
        .await
        .is_err()
    );
    assert!(
        peer.reserve_provider_call(
            &recovered,
            ReservePlanCall {
                limits: limits(2),
                ..request()
            }
        )
        .await
        .is_err()
    );
    let child = bound
        .capture_provider_child(
            &contexts,
            PlanChildCapture {
                root: &root,
                parent: &root,
                call_site: &site("metrics"),
                chain_path: &["diagnose".into()],
                action: &action,
                selected: &selected,
                admission_key: reserved.admission_key(),
                handle: reserved.handle().clone(),
                execution_id: reserved.execution_id(),
                permits: &refs(),
                limits: limits(1),
                clock: clock.as_ref(),
            },
        )
        .await
        .unwrap();
    let replay_child = bound
        .capture_provider_child(
            &contexts,
            PlanChildCapture {
                root: &root,
                parent: &root,
                call_site: &site("metrics"),
                chain_path: &["diagnose".into()],
                action: &action,
                selected: &selected,
                admission_key: reserved.admission_key(),
                handle: ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                permits: &refs(),
                limits: limits(1),
                clock: clock.as_ref(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        replay_child.reference().unwrap(),
        child.reference().unwrap()
    );
    let forged: Arc<dyn DynProvider> = Arc::new(Counter {
        calls: AtomicUsize::new(0),
    });
    assert!(
        bound
            .capture_provider_child(
                &contexts,
                PlanChildCapture {
                    root: &root,
                    parent: &root,
                    call_site: &site("metrics"),
                    chain_path: &["diagnose".into()],
                    action: &action,
                    selected: &forged,
                    admission_key: "forged-provider",
                    handle: ExecutionContextHandle::new(),
                    execution_id: uuid::Uuid::new_v4(),
                    permits: &refs(),
                    limits: limits(1),
                    clock: clock.as_ref(),
                }
            )
            .await
            .is_err()
    );
    assert_eq!(c.snapshot().await.unwrap().roots.len(), 2);
    let driver = GovernedProviderExecutor::new(
        state.clone(),
        c.clone(),
        contexts.clone(),
        catalog.resolve(&action, &selected).unwrap().clone(),
        ExecutorConfig {
            max_retries: 0,
            ..Default::default()
        },
        clock.clone(),
        None,
    )
    .unwrap();
    let call_site = site("metrics");
    let path = vec!["diagnose".into()];
    let permits = refs();
    let admission = bound.provider_admission(
        &contexts,
        PlanProviderAdmission {
            root: &root,
            parent: &root,
            call_site: &call_site,
            chain_path: &path,
            admission_key: reserved.admission_key(),
            handle: child.handle().clone(),
            execution_id: child.execution_id(),
            permits: &permits,
            limits: limits(1),
            clock: clock.as_ref(),
        },
    );
    assert!(
        admission
            .admit(ProviderInvocation {
                action: &action,
                selected: &selected,
                context: None,
                origin: ProviderInvocationOrigin::Dispatch,
                authority: None,
            })
            .await
            .is_err()
    );
    let authority = admission
        .admit(ProviderInvocation {
            action: &action,
            selected: &selected,
            context: None,
            origin: ProviderInvocationOrigin::ChainStep,
            authority: None,
        })
        .await
        .unwrap();
    let mediated_driver = GovernedProviderExecutor::new(
        state.clone(),
        c.clone(),
        contexts.clone(),
        catalog.resolve(&action, &selected).unwrap().clone(),
        ExecutorConfig {
            max_retries: 0,
            ..Default::default()
        },
        clock.clone(),
        None,
    )
    .unwrap();
    let mediator = GovernedProviderMediator::new(vec![mediated_driver]).unwrap();
    assert!(matches!(
        mediator
            .execute(ProviderInvocation {
                action: &action,
                selected: &selected,
                context: None,
                origin: ProviderInvocationOrigin::ChainStep,
                authority: Some(&authority),
            })
            .await,
        ActionOutcome::Executed(_)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let receipt = driver
        .execute(&child.reference().unwrap(), &refs(), &action, &actor())
        .await
        .unwrap();
    assert!(matches!(
        receipt.status,
        GovernedProviderStatus::Completed {
            outcome: ActionOutcome::Executed(_)
        }
    ));
    let replay = driver
        .execute(&child.reference().unwrap(), &refs(), &action, &actor())
        .await
        .unwrap();
    assert!(matches!(
        replay.status,
        GovernedProviderStatus::Completed { .. }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let next = Action::new(
        "city",
        "tenant",
        "incident",
        "execute",
        json!({"source":"logs"}),
    );
    let next_context = bound
        .capture_provider_child(
            &contexts,
            PlanChildCapture {
                root: &root,
                parent: &root,
                call_site: &site("logs"),
                chain_path: &["diagnose".into()],
                action: &next,
                selected: &selected,
                admission_key: "logs:1",
                handle: ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                permits: &refs(),
                limits: limits(1),
                clock: clock.as_ref(),
            },
        )
        .await
        .unwrap();
    let chain = ResourceRef::new(ResourceKind::Chain, "city", "tenant", "diagnose").unwrap();
    c.change(
        "close-chain",
        AuthorityChange::CloseResource { resource: chain },
        "operator",
        "closure",
    )
    .await
    .unwrap();
    let result = driver
        .execute(&next_context.reference().unwrap(), &refs(), &next, &actor())
        .await;
    assert!(
        result.is_err()
            || matches!(
                result.unwrap().status,
                GovernedProviderStatus::Completed {
                    outcome: ActionOutcome::Failed(_)
                }
            )
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let snapshot = c.snapshot().await.unwrap();
    assert_eq!(
        snapshot.roots[&root.execution_id().to_string()].spent_units,
        1
    );
    assert!(snapshot.roots.values().all(|r| r.active_attempts == 0));
    let chain = ResourceRef::new(ResourceKind::Chain, "city", "tenant", "diagnose").unwrap();
    c.change(
        "reopen-chain",
        AuthorityChange::ReopenResource { resource: chain },
        "operator",
        "resume",
    )
    .await
    .unwrap();
    let resumed = bound
        .capture_provider_child(
            &contexts,
            PlanChildCapture {
                root: &root,
                parent: &root,
                call_site: &site("logs"),
                chain_path: &["diagnose".into()],
                action: &next,
                selected: &selected,
                admission_key: "logs:2",
                handle: ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                permits: &refs(),
                limits: limits(1),
                clock: clock.as_ref(),
            },
        )
        .await
        .unwrap();
    let resumed_result = driver
        .execute(&resumed.reference().unwrap(), &refs(), &next, &actor())
        .await
        .unwrap();
    assert!(matches!(
        resumed_result.status,
        GovernedProviderStatus::Completed {
            outcome: ActionOutcome::Executed(_)
        }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        c.snapshot().await.unwrap().roots[&root.execution_id().to_string()].spent_units,
        2
    );
    let mut changed_definition = definitions();
    changed_definition.get_mut("diagnose").unwrap().version = 2;
    let changed_plan = Arc::new(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &changed_definition,
            catalog.clone(),
        )
        .unwrap(),
    );
    assert!(
        changed_plan
            .bind_input(&origin())
            .unwrap()
            .verify_root(&root)
            .is_err()
    );
    let incomplete = contexts
        .capture_permitted_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor(),
                    request_digest: bound.request_digest().into(),
                },
                credential_id: "private-key".into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: permit_revision_tag(&refs()).unwrap(),
                accepted_effects: plan
                    .required_effects()
                    .iter()
                    .filter(|e| e.operation != "chain.start")
                    .cloned()
                    .collect(),
                deadline_ms: 1000,
                evaluated_authority: c.snapshot().await.unwrap().stamp(),
            },
            &refs(),
            limits(2),
            clock.as_ref(),
        )
        .await
        .unwrap();
    assert!(bound.verify_root(&incomplete).is_err());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    // Expiry does not erase historical work, and recovering the pinned job
    // cannot renew its deadline or establish permission for another child.
    clock.advance_to(std::time::Duration::from_secs(2)).unwrap();
    let expired_job = peer
        .recover(job_id, catalog.clone(), &contexts)
        .await
        .unwrap();
    assert_eq!(
        expired_job.root().reference().unwrap(),
        root.reference().unwrap()
    );
    assert!(
        expired_job
            .invocation()
            .capture_provider_child(
                &contexts,
                PlanChildCapture {
                    root: expired_job.root(),
                    parent: expired_job.root(),
                    call_site: &site("logs"),
                    chain_path: &["diagnose".into()],
                    action: &action,
                    selected: &selected,
                    admission_key: "expired-attempt",
                    handle: ExecutionContextHandle::new(),
                    execution_id: uuid::Uuid::new_v4(),
                    permits: expired_job.permits(),
                    limits: limits(1),
                    clock: clock.as_ref(),
                }
            )
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    // Historical reads survive route retirement and preserve inherited root stops.
    let archive = acteon_executor::governed::history::HistoricalProviderStore::new(
        state.clone(),
        c.clone(),
        contexts.clone(),
        None,
    );
    c.change(
        "cancel-historical-root",
        AuthorityChange::CancelExecution {
            execution_id: root.execution_id().to_string(),
        },
        "operator",
        "retire parent job",
    )
    .await
    .unwrap();
    let historical = archive
        .inspect(&child.reference().unwrap(), &actor())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        historical.receipt.status,
        GovernedProviderStatus::Completed { .. }
    ));
    assert!(historical.cancellation_fenced);
    assert!(!c.snapshot().await.unwrap().roots[&child.execution_id().to_string()].cancelled);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    // A durable context cannot drop the chain closure by relabeling its work record.
    let storage = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
        next_context.reference().unwrap().context_id().to_string(),
    );
    let mut envelope: serde_json::Value =
        serde_json::from_str(&state.get(&storage).await.unwrap().unwrap()).unwrap();
    let mut payload: serde_json::Value =
        serde_json::from_str(envelope["payload"].as_str().unwrap()).unwrap();
    payload["lineage"]["restrictions"] = json!([]);
    envelope["payload"] = payload.to_string().into();
    state
        .set(&storage, &envelope.to_string(), None)
        .await
        .unwrap();
    assert!(
        contexts
            .recover_reference(&next_context.reference().unwrap(), 100)
            .await
            .is_err()
    );
}

#[test]
fn parallel_subchain_and_cancellation_footprints_are_complete_and_bounded() {
    let selected: Arc<dyn DynProvider> = Arc::new(Counter {
        calls: AtomicUsize::new(0),
    });
    let group = acteon_core::ParallelStepGroup {
        steps: vec![
            ChainStepConfig::new("metrics", "incident", "execute", json!({})),
            ChainStepConfig::new("logs", "incident", "execute", json!({})),
        ],
        join: acteon_core::ParallelJoinPolicy::default(),
        on_failure: acteon_core::ParallelFailurePolicy::default(),
        timeout_seconds: None,
        max_concurrency: None,
    };
    let mut nested = definitions();
    nested.get_mut("diagnose").unwrap().steps =
        vec![ChainStepConfig::new_sub_chain("inspect", "inspect")];
    nested.insert(
        "inspect".into(),
        ChainConfig::new("inspect")
            .with_step(ChainStepConfig::new_parallel("fanout", group))
            .with_on_cancel(acteon_core::ChainNotificationTarget {
                provider: "incident".into(),
                action_type: "execute".into(),
            }),
    );
    let plan = QualifiedChainPlan::new_trusted(
        "city",
        "tenant",
        "diagnose",
        &nested,
        catalog(selected.clone(), "v1"),
    )
    .unwrap();
    assert_eq!(plan.required_effects().len(), 3);
    assert_eq!(plan.definitions().len(), 2);
    let mut wide = definitions();
    wide.get_mut("diagnose").unwrap().steps = (0..129)
        .map(|i| ChainStepConfig::new(format!("step-{i}"), "incident", "execute", json!({})))
        .collect();
    assert!(matches!(
        QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &wide,
            catalog(selected, "v1")
        ),
        Err(PlanError::Capacity)
    ));
}

#[test]
fn unimplemented_worker_and_full_dispatch_adapters_fail_qualification() {
    let selected: Arc<dyn DynProvider> = Arc::new(Counter {
        calls: AtomicUsize::new(0),
    });
    for step in [
        ChainStepConfig::new_worker(
            "external",
            acteon_core::WorkerStepConfig {
                queue: "workers".into(),
                action_type: None,
                timeout_seconds: None,
                max_attempts: None,
            },
            json!({}),
        ),
        ChainStepConfig::new_dispatch(
            "pipeline",
            acteon_core::DispatchStepConfig {
                provider: "incident".into(),
                action_type: "execute".into(),
                dedup_key: None,
                inherit_metadata: true,
            },
            json!({}),
        ),
    ] {
        let source = BTreeMap::from([(
            "diagnose".into(),
            ChainConfig::new("diagnose").with_step(step),
        )]);
        assert!(matches!(
            QualifiedChainPlan::new_trusted(
                "city",
                "tenant",
                "diagnose",
                &source,
                catalog(selected.clone(), "v1")
            ),
            Err(PlanError::Unsupported)
        ));
    }
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL"]
async fn independent_redis_qualified_handoff_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("plan-handoff-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    Box::pin(qualified_handoff_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        Arc::new(RedisStateStore::new(&config).unwrap()),
        true,
    ))
    .await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn independent_postgres_qualified_handoff_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("plan_handoff_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    Box::pin(qualified_handoff_contract(
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        true,
    ))
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
async fn encrypted_qualified_job_and_call_handoff_survive_restart() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    Box::pin(qualified_handoff_contract(state.clone(), state, true)).await;
}

#[cfg(feature = "dynamodb")]
#[tokio::test]
#[ignore = "requires DYNAMODB_ENDPOINT; independent DynamoDB Local clients"]
async fn independent_dynamodb_qualified_handoff_contract() {
    use acteon_state_dynamodb::{DynamoConfig, DynamoStateStore, build_client, create_table};
    let config = DynamoConfig {
        endpoint_url: Some(
            std::env::var("DYNAMODB_ENDPOINT").expect("DynamoDB Local endpoint required"),
        ),
        table_name: format!("plan_handoff_{}", uuid::Uuid::new_v4().simple()),
        key_prefix: format!("plan_handoff_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let client = build_client(&config).await;
    create_table(&client, &config.table_name).await.unwrap();
    Box::pin(qualified_handoff_contract(
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
        true,
    ))
    .await;
    client
        .delete_table()
        .table_name(&config.table_name)
        .send()
        .await
        .unwrap();
}
