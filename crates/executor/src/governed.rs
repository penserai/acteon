//! Durable, directly mediated provider execution under exact root permits.
//! This does not install gateway/server-wide policy or govern provider internals.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use acteon_core::{
    Action, ActionError, ActionOutcome, ExecutionContextReference, PrincipalIdentity, ResourceKind,
    ResourceRef,
};
use acteon_crypto::PayloadEncryptor;
use acteon_governance::context::{AcceptedEffect, TrustedContextStore};
use acteon_governance::permit::{
    PermitReference, PermittedAttempt, permit_revision_tag, permitted_attempt_digest,
};
use acteon_governance::{
    AttemptEvidenceReference, AttemptStatus, AuthorityCoordinator, CoordinationError,
    StartRegistration,
};
use acteon_provider::{DynProvider, ProviderError};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_time::Clock;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    ActionExecutor, AttemptGateError, AttemptSettlement, ExecutorConfig, ProviderAttempt,
    ProviderAttemptGate, ProviderAttemptOutcome, RegisteredProviderAttempt, RetryStrategy,
};

pub const OPERATION_KIND: &str = "governed_provider_operation";
pub const RESULT_KIND: &str = "governed_provider_result";
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ATTEMPTS: u32 = 32;

#[derive(Debug, thiserror::Error)]
pub enum GovernedProviderError {
    #[error("invalid or unsupported governed provider definition")]
    Invalid,
    #[error("governed operation ownership or binding mismatch")]
    Ownership,
    #[error("governed operation conflicts with its durable definition")]
    Conflict,
    #[error("governed execution storage or verification unavailable")]
    Unavailable,
    #[error("governed attempt admission refused: {0}")]
    Admission(&'static str),
}

/// Qualified provider-specific evidence that an error had no external effect.
/// Generic connection/timeouts are ambiguous. Hosts must review this contract;
/// prompts, registry cards and client fields cannot establish it.
pub trait ProviderFailureContract: Send + Sync {
    fn revision(&self) -> &str;
    fn known_rejected(&self, error: &ProviderError) -> bool;
}
struct ConservativeFailures;
impl ProviderFailureContract for ConservativeFailures {
    fn revision(&self) -> &'static str {
        "uncertain-v1"
    }
    fn known_rejected(&self, _: &ProviderError) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    provider: String,
    effect: AcceptedEffect,
    revision: String,
    failure_revision: String,
}

/// Privileged host binding. Endpoint identity/version must describe the actual
/// immutable provider instance and all additional protected resources. A provider
/// with dynamic destinations/internal retries needs a qualified resolver first.
#[derive(Clone)]
pub struct BoundProvider {
    provider: Arc<dyn DynProvider>,
    binding: Binding,
    failures: Arc<dyn ProviderFailureContract>,
    action_type: String,
    endpoint: ResourceRef,
    catalog_version: Option<ResourceRef>,
}
impl BoundProvider {
    pub fn new_trusted(
        provider: Arc<dyn DynProvider>,
        endpoint: &ResourceRef,
        action_type: &str,
        revision: &str,
        additional: Vec<ResourceRef>,
    ) -> Result<Self, GovernedProviderError> {
        if endpoint.kind() != ResourceKind::Endpoint
            || !valid_text(revision)
            || !valid_text(action_type)
        {
            return Err(GovernedProviderError::Invalid);
        }
        let mut resources = vec![
            ResourceRef::new(
                ResourceKind::Provider,
                endpoint.namespace(),
                endpoint.tenant(),
                provider.name(),
            )
            .map_err(|_| GovernedProviderError::Invalid)?,
            endpoint.clone(),
            ResourceRef::new(
                ResourceKind::Action,
                endpoint.namespace(),
                endpoint.tenant(),
                action_type,
            )
            .map_err(|_| GovernedProviderError::Invalid)?,
        ];
        resources.extend(additional);
        resources.sort();
        if resources.len() > 16
            || resources.windows(2).any(|r| r[0] == r[1])
            || resources
                .iter()
                .any(|r| r.namespace() != endpoint.namespace() || r.tenant() != endpoint.tenant())
        {
            return Err(GovernedProviderError::Invalid);
        }
        Ok(Self {
            binding: Binding {
                provider: provider.name().into(),
                effect: AcceptedEffect {
                    operation: "provider.execute".into(),
                    resources,
                },
                revision: revision.into(),
                failure_revision: "uncertain-v1".into(),
            },
            provider,
            failures: Arc::new(ConservativeFailures),
            action_type: action_type.into(),
            endpoint: endpoint.clone(),
            catalog_version: None,
        })
    }
    pub fn with_failure_contract(
        mut self,
        contract: Arc<dyn ProviderFailureContract>,
    ) -> Result<Self, GovernedProviderError> {
        if !valid_text(contract.revision()) {
            return Err(GovernedProviderError::Invalid);
        }
        self.binding.failure_revision = contract.revision().into();
        self.failures = contract;
        if self.catalog_version.is_some() {
            self.refresh_catalog_version()?;
        }
        Ok(self)
    }
    #[must_use]
    pub fn effect(&self) -> &AcceptedEffect {
        &self.binding.effect
    }

    pub(crate) fn catalog_definition(&self) -> crate::catalog::QualifiedProviderDefinition {
        crate::catalog::QualifiedProviderDefinition {
            namespace: self.endpoint.namespace().into(),
            tenant: self.endpoint.tenant().into(),
            provider: self.binding.provider.clone(),
            action_type: self.action_type.clone(),
            endpoint: self.endpoint.clone(),
            revision: self.binding.revision.clone(),
            failure_revision: self.binding.failure_revision.clone(),
            effect: self.binding.effect.clone(),
        }
    }

    pub(crate) fn is_provider(&self, provider: &Arc<dyn DynProvider>) -> bool {
        Arc::ptr_eq(&self.provider, provider)
    }

    /// Protect the complete trusted binding definition with a version route.
    /// Qualification is idempotent. The host remains responsible for reviewing
    /// the provider's immutable settings and failure contract before calling.
    pub fn qualify_for_catalog(mut self) -> Result<Self, GovernedProviderError> {
        if self.catalog_version.is_none() {
            self.refresh_catalog_version()?;
        }
        Ok(self)
    }

    fn refresh_catalog_version(&mut self) -> Result<(), GovernedProviderError> {
        if let Some(previous) = self.catalog_version.take() {
            self.binding.effect.resources.retain(|r| r != &previous);
        }
        if self.binding.effect.resources.len() >= 16 {
            return Err(GovernedProviderError::Invalid);
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "format": "acteon.qualified_provider.v1", "binding": self.binding,
            "primary_action": self.action_type, "endpoint": self.endpoint,
        }))
        .map_err(|_| GovernedProviderError::Invalid)?;
        let version = ResourceRef::new(
            ResourceKind::Route,
            self.endpoint.namespace(),
            self.endpoint.tenant(),
            format!("provider-version/{:x}", Sha256::digest(bytes)),
        )
        .map_err(|_| GovernedProviderError::Invalid)?;
        self.binding.effect.resources.push(version.clone());
        self.binding.effect.resources.sort();
        self.catalog_version = Some(version);
        Ok(())
    }
}
fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let ordered: BTreeMap<_, _> = map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            serde_json::Value::Object(ordered.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}
/// Semantic work binding. Delivery IDs, timestamps, tracing and signatures may
/// rotate; the first durable Action is retained and supplied to the provider.
pub fn governed_provider_input_digest(action: &Action) -> Result<String, GovernedProviderError> {
    let mut value = serde_json::to_value(action).map_err(|_| GovernedProviderError::Invalid)?;
    for field in [
        "id",
        "created_at",
        "trace_context",
        "signature",
        "kid",
        "signer_id",
    ] {
        value
            .as_object_mut()
            .ok_or(GovernedProviderError::Invalid)?
            .remove(field);
    }
    let raw = serde_json::to_vec(&canonical(value)).map_err(|_| GovernedProviderError::Invalid)?;
    Ok(format!("{:x}", Sha256::digest(raw)))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    max_attempts: u32,
    delays_ns: Vec<u64>,
    timeout_ns: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    schema: u32,
    context: ExecutionContextReference,
    permits: Vec<PermitReference>,
    action: Action,
    binding: Binding,
    settings: Settings,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema: u32,
    context: ExecutionContextReference,
    binding: Binding,
    ordinal: u32,
    token: String,
    outcome: ActionOutcome,
    known: bool,
    retry_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GovernedProviderStatus {
    Prepared,
    InFlight { attempt_id: String },
    AwaitingRetry { not_before_ms: i64 },
    Completed { outcome: ActionOutcome },
    ReconciliationRequired { attempt_id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedProviderReceipt {
    pub execution_id: Uuid,
    pub attempts: u32,
    pub status: GovernedProviderStatus,
}

struct Runtime {
    state: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    contexts: Arc<TrustedContextStore>,
    bound: BoundProvider,
    settings: Settings,
    clock: Arc<dyn Clock>,
    encryptor: Option<Arc<PayloadEncryptor>>,
}
/// Direct provider boundary, not the rules/approval/chain dispatch pipeline.
/// Authentication and qualification are host prerequisites; no public server
/// enforce switch is enabled by constructing this object.
pub struct GovernedProviderExecutor {
    require_credential_authority: bool,
    runtime: Arc<Runtime>,
    executor: ActionExecutor,
}

fn operation_key(reference: &ExecutionContextReference) -> StateKey {
    StateKey::new(
        reference.namespace(),
        reference.tenant(),
        KeyKind::Custom(OPERATION_KIND.into()),
        reference.execution_id().to_string(),
    )
}
fn attempt_id(root: Uuid, ordinal: u32) -> String {
    Uuid::new_v5(&root, &ordinal.to_be_bytes()).to_string()
}
fn result_key(reference: &ExecutionContextReference, ordinal: u32) -> StateKey {
    StateKey::new(
        reference.namespace(),
        reference.tenant(),
        KeyKind::Custom(RESULT_KIND.into()),
        attempt_id(reference.execution_id(), ordinal),
    )
}

impl Runtime {
    fn encode<T: Serialize>(&self, value: &T) -> Result<String, GovernedProviderError> {
        let raw = serde_json::to_string(value).map_err(|_| GovernedProviderError::Invalid)?;
        if raw.len() > MAX_BYTES {
            return Err(GovernedProviderError::Invalid);
        }
        match &self.encryptor {
            Some(e) => e
                .encrypt_str(&raw)
                .map_err(|_| GovernedProviderError::Unavailable),
            None => Ok(raw),
        }
    }
    fn decode<T: serde::de::DeserializeOwned>(
        &self,
        encoded: &str,
    ) -> Result<(T, String), GovernedProviderError> {
        if encoded.len() > MAX_BYTES * 2 {
            return Err(GovernedProviderError::Invalid);
        }
        let raw = match &self.encryptor {
            Some(e) => e
                .decrypt_str(encoded)
                .map_err(|_| GovernedProviderError::Unavailable)?,
            None => encoded.into(),
        };
        if raw.len() > MAX_BYTES {
            return Err(GovernedProviderError::Invalid);
        }
        let digest = format!("{:x}", Sha256::digest(raw.as_bytes()));
        Ok((
            serde_json::from_str(&raw).map_err(|_| GovernedProviderError::Invalid)?,
            digest,
        ))
    }
    async fn load_operation(
        &self,
        reference: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<Operation>, GovernedProviderError> {
        if reference.principal() != actor {
            return Err(GovernedProviderError::Ownership);
        }
        let Some(raw) = self
            .state
            .get(&operation_key(reference))
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            return Ok(None);
        };
        let (op, _): (Operation, _) = self.decode(&raw)?;
        if op.schema != 1
            || op.context != *reference
            || op.binding != self.bound.binding
            || op.settings != self.settings
            || op.action.namespace.as_str() != reference.namespace()
            || op.action.tenant.as_str() != reference.tenant()
            || governed_provider_input_digest(&op.action)? != reference.request_digest()
        {
            return Err(GovernedProviderError::Conflict);
        }
        let verified = self
            .contexts
            .recover_reference_for_observation(&op.context)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if permit_revision_tag(&op.permits).map_err(|_| GovernedProviderError::Invalid)?
            != verified.accepted_ceiling_revision()
        {
            return Err(GovernedProviderError::Conflict);
        }
        Ok(Some(op))
    }
    async fn load_evidence(
        &self,
        op: &Operation,
        ordinal: u32,
        token: &str,
    ) -> Result<Option<(Evidence, AttemptEvidenceReference)>, GovernedProviderError> {
        let key = result_key(&op.context, ordinal);
        let Some(raw) = self
            .state
            .get(&key)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?
        else {
            return Ok(None);
        };
        let (e, digest): (Evidence, _) = self.decode(&raw)?;
        if e.schema != 1
            || e.context != op.context
            || e.binding != op.binding
            || e.ordinal != ordinal
            || e.token != token
            || (e.retry_at_ms.is_some()
                && (!e.known || !matches!(e.outcome, ActionOutcome::Failed(_))))
        {
            return Err(GovernedProviderError::Conflict);
        }
        Ok(Some((
            e,
            AttemptEvidenceReference {
                id: key.id.clone(),
                digest,
            },
        )))
    }
    async fn observe(
        &self,
        op: &Operation,
    ) -> Result<GovernedProviderReceipt, GovernedProviderError> {
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let verified = self
            .contexts
            .recover_reference_for_observation(&op.context)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if verified.authority_stamp().incarnation != snapshot.incarnation {
            return Err(GovernedProviderError::Conflict);
        }
        let mut receipt = GovernedProviderReceipt {
            execution_id: op.context.execution_id(),
            attempts: 0,
            status: GovernedProviderStatus::Prepared,
        };
        for ordinal in 0..op.settings.max_attempts {
            let id = attempt_id(op.context.execution_id(), ordinal);
            let Some(start) = snapshot.starts.get(&id) else {
                return Ok(receipt);
            };
            if start.request_digest
                != permitted_attempt_digest(&op.context, &op.binding.effect, 1, &op.permits)
                    .map_err(|_| GovernedProviderError::Conflict)?
                || start.subject != op.context.principal().id()
                || start
                    .resources
                    .iter()
                    .ne(op.binding.effect.resources.iter())
                || start.reservation.as_ref().is_none_or(|r| {
                    r.root_id != op.context.execution_id().to_string() || r.units != 1
                })
            {
                return Err(GovernedProviderError::Conflict);
            }
            receipt.attempts = ordinal + 1;
            let Some((e, reference)) = self.load_evidence(op, ordinal, &start.token).await? else {
                receipt.status = if start.status == AttemptStatus::InFlight {
                    GovernedProviderStatus::InFlight { attempt_id: id }
                } else {
                    GovernedProviderStatus::ReconciliationRequired { attempt_id: id }
                };
                return Ok(receipt);
            };
            if start
                .evidence
                .as_ref()
                .is_some_and(|prior| prior != &reference)
            {
                return Err(GovernedProviderError::Conflict);
            }
            let status = if e.known {
                AttemptStatus::Settled
            } else {
                AttemptStatus::Uncertain
            };
            self.coordinator
                .settle_with_evidence(&id, &start.token, status, reference)
                .await
                .map_err(|_| GovernedProviderError::Unavailable)?;
            if !e.known {
                receipt.status = GovernedProviderStatus::ReconciliationRequired { attempt_id: id };
                return Ok(receipt);
            }
            if let Some(not_before_ms) = e.retry_at_ms {
                receipt.status = GovernedProviderStatus::AwaitingRetry { not_before_ms };
            } else {
                receipt.status = GovernedProviderStatus::Completed { outcome: e.outcome };
                return Ok(receipt);
            }
        }
        Ok(receipt)
    }
}

impl GovernedProviderExecutor {
    pub(crate) fn bound_provider(&self) -> &BoundProvider {
        &self.runtime.bound
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Arc<dyn StateStore>,
        coordinator: AuthorityCoordinator,
        contexts: Arc<TrustedContextStore>,
        bound: BoundProvider,
        config: ExecutorConfig,
        clock: Arc<dyn Clock>,
        encryptor: Option<Arc<PayloadEncryptor>>,
    ) -> Result<Self, GovernedProviderError> {
        if config.max_retries >= MAX_ATTEMPTS
            || config.max_concurrent == 0
            || config.execution_timeout.is_zero()
        {
            return Err(GovernedProviderError::Invalid);
        }
        let settings = Settings {
            max_attempts: config.max_retries + 1,
            delays_ns: (0..config.max_retries)
                .map(|n| {
                    u64::try_from(config.retry_strategy.delay_for(n).as_nanos())
                        .map_err(|_| GovernedProviderError::Invalid)
                })
                .collect::<Result<_, _>>()?,
            timeout_ns: u64::try_from(config.execution_timeout.as_nanos())
                .map_err(|_| GovernedProviderError::Invalid)?,
        };
        let mut execution = config;
        execution.retry_strategy = RetryStrategy::Constant {
            delay: Duration::ZERO,
        };
        Ok(Self {
            require_credential_authority: false,
            runtime: Arc::new(Runtime {
                state,
                coordinator,
                contexts,
                bound,
                settings,
                clock: clock.clone(),
                encryptor,
            }),
            executor: ActionExecutor::new(execution)
                .clock(clock)
                .require_attempt_gate(),
        })
    }
    /// Refuse execution under actor-only compatibility contexts. Hosts exposing
    /// credential-governed work must use credentialed capture and this profile.
    #[must_use]
    pub fn require_credential_authority(mut self) -> Self {
        self.require_credential_authority = true;
        self
    }
    /// Replay/repair known evidence without execution, including after expiry.
    pub async fn inspect(
        &self,
        reference: &ExecutionContextReference,
        actor: &PrincipalIdentity,
    ) -> Result<Option<GovernedProviderReceipt>, GovernedProviderError> {
        let Some(op) = self.runtime.load_operation(reference, actor).await? else {
            return Ok(None);
        };
        Ok(Some(self.runtime.observe(&op).await?))
    }
    /// Caller identity is independently authenticated by the host. Root context
    /// and endpoint binding cannot be derived from untrusted SDK metadata.
    pub async fn execute(
        &self,
        reference: &ExecutionContextReference,
        permits: &[PermitReference],
        action: &Action,
        actor: &PrincipalIdentity,
    ) -> Result<GovernedProviderReceipt, GovernedProviderError> {
        if actor != reference.principal()
            || action.namespace.as_str() != reference.namespace()
            || action.tenant.as_str() != reference.tenant()
            || governed_provider_input_digest(action)? != reference.request_digest()
        {
            return Err(GovernedProviderError::Ownership);
        }
        if action.template.is_some() || !action.attachments.is_empty() {
            return Err(GovernedProviderError::Invalid);
        }
        let effect = &self.runtime.bound.binding.effect;
        if action.action_type != self.runtime.bound.action_type
            || effect
                .resources
                .iter()
                .any(|r| r.namespace() != reference.namespace() || r.tenant() != reference.tenant())
        {
            return Err(GovernedProviderError::Invalid);
        }
        let verified = self
            .runtime
            .contexts
            .recover_reference_for_observation(reference)
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        if self.require_credential_authority && verified.credential_authority().is_none() {
            return Err(GovernedProviderError::Admission(
                "CREDENTIAL_AUTHORITY_REQUIRED",
            ));
        }
        if permit_revision_tag(permits).map_err(|_| GovernedProviderError::Invalid)?
            != verified.accepted_ceiling_revision()
        {
            return Err(GovernedProviderError::Ownership);
        }
        let mut selected = permits.to_vec();
        selected.sort_by(|a, b| a.id.cmp(&b.id));
        let proposed = Operation {
            schema: 1,
            context: reference.clone(),
            permits: selected,
            action: action.clone(),
            binding: self.runtime.bound.binding.clone(),
            settings: self.runtime.settings.clone(),
        };
        self.runtime
            .state
            .check_and_set(
                &operation_key(reference),
                &self.runtime.encode(&proposed)?,
                None,
            )
            .await
            .map_err(|_| GovernedProviderError::Unavailable)?;
        let op = self
            .runtime
            .load_operation(reference, actor)
            .await?
            .ok_or(GovernedProviderError::Unavailable)?;
        if op.permits != proposed.permits {
            return Err(GovernedProviderError::Conflict);
        }
        let before = self.runtime.observe(&op).await?;
        let base = match before.status {
            GovernedProviderStatus::Prepared => 0,
            GovernedProviderStatus::AwaitingRetry { .. } => before.attempts,
            _ => return Ok(before),
        };
        let refused = Arc::new(Mutex::new(None));
        let gate = Gate {
            runtime: self.runtime.clone(),
            op: op.clone(),
            base,
            refused: refused.clone(),
        };
        let _outcome = self
            .executor
            .execute_with_gate(
                &op.action,
                self.runtime.bound.provider.as_ref(),
                None,
                &gate,
            )
            .await;
        let result = self.runtime.observe(&op).await?;
        if matches!(
            result.status,
            GovernedProviderStatus::Prepared | GovernedProviderStatus::AwaitingRetry { .. }
        ) && let Some(error) = *refused
            .lock()
            .map_err(|_| GovernedProviderError::Unavailable)?
        {
            return Err(GovernedProviderError::Admission(error.code()));
        }
        Ok(result)
    }
}

struct Gate {
    runtime: Arc<Runtime>,
    op: Operation,
    base: u32,
    refused: Arc<Mutex<Option<AttemptGateError>>>,
}
struct Guard {
    runtime: Arc<Runtime>,
    op: Operation,
    ordinal: u32,
    id: String,
    token: String,
}
fn admission_error(error: &CoordinationError) -> AttemptGateError {
    match error {
        CoordinationError::State(_) => AttemptGateError::Unavailable,
        _ => AttemptGateError::Denied,
    }
}
#[async_trait]
impl ProviderAttemptGate for Gate {
    async fn start(
        &self,
        attempt: ProviderAttempt<'_>,
    ) -> Result<Box<dyn RegisteredProviderAttempt>, AttemptGateError> {
        let result = self.start_inner(attempt).await;
        if let Err(error) = &result {
            *self
                .refused
                .lock()
                .map_err(|_| AttemptGateError::Unavailable)? = Some(*error);
        }
        result
    }
}
impl Gate {
    async fn start_inner(
        &self,
        attempt: ProviderAttempt<'_>,
    ) -> Result<Box<dyn RegisteredProviderAttempt>, AttemptGateError> {
        let ordinal = self
            .base
            .checked_add(attempt.ordinal)
            .filter(|n| *n < self.op.settings.max_attempts)
            .ok_or(AttemptGateError::Denied)?;
        if attempt.provider_name != self.op.binding.provider
            || governed_provider_input_digest(attempt.action)
                .map_err(|_| AttemptGateError::Conflict)?
                != self.op.context.request_digest()
        {
            return Err(AttemptGateError::Conflict);
        }
        if ordinal > 0 {
            let snapshot = self
                .runtime
                .coordinator
                .snapshot()
                .await
                .map_err(|_| AttemptGateError::Unavailable)?;
            let prior = snapshot
                .starts
                .get(&attempt_id(self.op.context.execution_id(), ordinal - 1))
                .ok_or(AttemptGateError::Conflict)?;
            let (e, reference) = self
                .runtime
                .load_evidence(&self.op, ordinal - 1, &prior.token)
                .await
                .map_err(|_| AttemptGateError::Unavailable)?
                .ok_or(AttemptGateError::Conflict)?;
            if prior.status != AttemptStatus::Settled
                || prior.evidence.as_ref() != Some(&reference)
                || !e.known
            {
                return Err(AttemptGateError::Conflict);
            }
            let due = e.retry_at_ms.ok_or(AttemptGateError::Conflict)?;
            let verified = self
                .runtime
                .contexts
                .recover_reference_for_observation(&self.op.context)
                .await
                .map_err(|_| AttemptGateError::Unavailable)?;
            let wake = due.min(verified.deadline_ms());
            let now = self.runtime.clock.now().timestamp_millis();
            if wake > now {
                self.runtime
                    .clock
                    .sleep(Duration::from_millis(
                        u64::try_from(wake - now).map_err(|_| AttemptGateError::Denied)?,
                    ))
                    .await;
            }
        }
        let context = self
            .runtime
            .contexts
            .recover_reference(
                &self.op.context,
                self.runtime.clock.now().timestamp_millis(),
            )
            .await
            .map_err(|_| AttemptGateError::Denied)?;
        let id = attempt_id(self.op.context.execution_id(), ordinal);
        let result = self
            .runtime
            .coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: &id,
                context: &context,
                permits: &self.op.permits,
                effect: &self.op.binding.effect,
                request_digest: self.op.context.request_digest(),
                units: 1,
                clock: self.runtime.clock.as_ref(),
            })
            .await
            .map_err(|error| admission_error(&error))?;
        let StartRegistration::New(record) = result else {
            return Err(AttemptGateError::Conflict);
        };
        Ok(Box::new(Guard {
            runtime: self.runtime.clone(),
            op: self.op.clone(),
            ordinal,
            id,
            token: record.token,
        }))
    }
}
#[async_trait]
impl RegisteredProviderAttempt for Guard {
    async fn finish(
        self: Box<Self>,
        outcome: ProviderAttemptOutcome<'_>,
    ) -> Result<AttemptSettlement, AttemptGateError> {
        let (outcome, known, retryable) = match outcome {
            ProviderAttemptOutcome::Succeeded(response) => {
                (ActionOutcome::Executed(response.clone()), true, false)
            }
            ProviderAttemptOutcome::Failed(error) => (
                ActionOutcome::Failed(ActionError {
                    code: "PROVIDER_ERROR".into(),
                    message: error.public_message(),
                    retryable: false,
                    attempts: self.ordinal + 1,
                }),
                self.runtime.bound.failures.known_rejected(error),
                error.is_retryable(),
            ),
            ProviderAttemptOutcome::TimedOut => (
                ActionOutcome::Failed(ActionError {
                    code: "TIMEOUT".into(),
                    message: "provider execution timed out".into(),
                    retryable: false,
                    attempts: self.ordinal + 1,
                }),
                false,
                false,
            ),
        };
        let retry_at_ms = if known && retryable && self.ordinal + 1 < self.op.settings.max_attempts
        {
            let ns = self.op.settings.delays_ns[self.ordinal as usize];
            let ms = u64::try_from(u128::from(ns).div_ceil(1_000_000))
                .map_err(|_| AttemptGateError::Unavailable)?;
            Some(
                self.runtime
                    .clock
                    .now()
                    .timestamp_millis()
                    .checked_add(i64::try_from(ms).map_err(|_| AttemptGateError::Unavailable)?)
                    .ok_or(AttemptGateError::Unavailable)?,
            )
        } else {
            None
        };
        let evidence = Evidence {
            schema: 1,
            context: self.op.context.clone(),
            binding: self.op.binding.clone(),
            ordinal: self.ordinal,
            token: self.token.clone(),
            outcome,
            known,
            retry_at_ms,
        };
        let key = result_key(&self.op.context, self.ordinal);
        self.runtime
            .state
            .check_and_set(
                &key,
                &self
                    .runtime
                    .encode(&evidence)
                    .map_err(|_| AttemptGateError::Unavailable)?,
                None,
            )
            .await
            .map_err(|_| AttemptGateError::Unavailable)?;
        let (saved, reference) = self
            .runtime
            .load_evidence(&self.op, self.ordinal, &self.token)
            .await
            .map_err(|_| AttemptGateError::Unavailable)?
            .ok_or(AttemptGateError::Unavailable)?;
        let status = if saved.known {
            AttemptStatus::Settled
        } else {
            AttemptStatus::Uncertain
        };
        self.runtime
            .coordinator
            .settle_with_evidence(&self.id, &self.token, status, reference)
            .await
            .map_err(|_| AttemptGateError::Unavailable)?;
        Ok(if saved.known {
            AttemptSettlement::Settled
        } else {
            AttemptSettlement::Uncertain
        })
    }
}
