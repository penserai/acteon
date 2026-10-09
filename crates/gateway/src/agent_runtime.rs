//! Durable individual-agent task execution on the existing governed provider boundary.
//!
//! This trusted host adapter accepts an already authenticated, signed recipient
//! context. It is not an A2A codec or permission to send a network request.
use std::sync::Arc;

use acteon_core::{
    Action, ActionOutcome, Artifact, ExecutionContextReference, TASK_CHALLENGE_ID_METADATA_KEY,
    Task, TaskAuthorizationRequirement, TaskMessage, TaskPart, TaskRole, TaskState,
};
use acteon_executor::{
    ExecutorConfig,
    delegation::ApprovedPeerBinding,
    governed::{
        BoundProvider, GovernedProviderError, GovernedProviderExecutor, GovernedProviderReceipt,
        GovernedProviderStatus, governed_provider_input_digest,
    },
};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, CoordinationError, RootBudgetLimits,
    context::{
        ChildContextAdmission, ContextError, ExecutionContextHandle, TrustedContextStore,
        VerifiedExecutionContext,
    },
    permit::{PermitReference, matches_effect, permit_revision_tag},
};
use acteon_state::{CasResult, KeyKind, StateError, StateKey, StateStore};
use acteon_time::Clock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{TaskAuthorizationVerifier, TaskEngine, TaskEngineError, TaskScope};

pub const ACCEPTANCE_KIND: &str = "governed_agent_task_acceptance";
pub const CONTINUATION_KIND: &str = "governed_agent_task_continuation";
pub const GOVERNED_TASK_METADATA_KEY: &str = "acteon_governed_execution";
const MAX_ACCEPTANCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTINUATION_CAS_ATTEMPTS: usize = 16;

pub struct AgentRuntimeDependencies {
    pub state: Arc<dyn StateStore>,
    pub coordinator: AuthorityCoordinator,
    pub contexts: Arc<TrustedContextStore>,
    pub clock: Arc<dyn Clock>,
}

/// Identify accepted governed work from its durable journal, including when a
/// task projection is missing or its display metadata has been damaged. This
/// observation supplies no authority to inspect or mutate the task itself.
pub async fn is_governed_agent_task(
    state: &dyn StateStore,
    namespace: &str,
    tenant: &str,
    task_id: &str,
) -> Result<bool, StateError> {
    state
        .get(&StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(ACCEPTANCE_KIND.into()),
            task_id,
        ))
        .await
        .map(|value| value.is_some())
}

#[derive(Debug, thiserror::Error)]
pub enum AgentRuntimeError {
    #[error("agent runtime input or binding is invalid")]
    Invalid,
    #[error("agent task conflicts with its original accepted work")]
    Conflict,
    #[error("accepted agent task is missing")]
    Missing,
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error(transparent)]
    Authority(#[from] CoordinationError),
    #[error(transparent)]
    Execution(#[from] GovernedProviderError),
    #[error(transparent)]
    Task(#[from] TaskEngineError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acceptance {
    schema: u32,
    binding_digest: String,
    reference: ExecutionContextReference,
    permits: Vec<PermitReference>,
    action: Action,
    initial_task: Task,
}

#[derive(Deserialize)]
struct AcceptanceRoutingHint {
    binding_digest: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    schema: u32,
    binding_digest: String,
    task_id: uuid::Uuid,
    challenge_id: String,
    parent: ExecutionContextReference,
    predecessor: ExecutionContextReference,
    child_handle: ExecutionContextHandle,
    child_execution_id: uuid::Uuid,
    limits: RootBudgetLimits,
    permits: Vec<PermitReference>,
    prior_task: Task,
    response_digest: String,
    response: TaskMessage,
    action: Action,
    state: ContinuationState,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
enum ContinuationState {
    Registered,
    Admitted {
        reference: ExecutionContextReference,
    },
}

struct CurrentOperation<'a> {
    reference: &'a ExecutionContextReference,
    action: &'a Action,
}

/// Read the bounded binding selector used by a trusted recovery host. This is
/// only a routing hint; the selected runtime must decode and verify the complete
/// immutable acceptance before returning data or starting an effect.
pub fn accepted_agent_binding_digest(raw: &str) -> Result<String, AgentRuntimeError> {
    if raw.len() > MAX_ACCEPTANCE_BYTES {
        return Err(AgentRuntimeError::Invalid);
    }
    let hint: AcceptanceRoutingHint = serde_json::from_str(raw)?;
    if hint.binding_digest.len() != 64
        || !hint
            .binding_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(AgentRuntimeError::Invalid);
    }
    Ok(hint.binding_digest)
}

/// An individual agent bound to one qualified provider operation. Other runtime
/// families (chains, workers, external runtimes) need their own adapters.
/// Requests cannot select providers, endpoints, executing identities, or plans.
pub struct AgentProviderRuntime {
    dependencies: AgentRuntimeDependencies,
    binding: ApprovedPeerBinding,
    bound: BoundProvider,
    executor: GovernedProviderExecutor,
    tasks: TaskEngine,
}

pub struct AgentTaskReceipt {
    pub task: Task,
    pub execution: Option<GovernedProviderReceipt>,
    /// Durable coordinator restriction, independent of provider outcome.
    pub future_starts_blocked: bool,
    /// A response intent exists and recovery may need to move a still-paused
    /// task into its governed continuation execution.
    pub continuation_pending: bool,
}

/// Acknowledgement of a restrictive control write, not a provider abort.
#[derive(Serialize)]
pub struct AgentTaskStopReceipt {
    pub task: Task,
    pub future_starts_blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_abort: Option<acteon_executor::governed::abort::ProviderAbortStatus>,
}

impl AgentProviderRuntime {
    pub fn new_trusted(
        dependencies: AgentRuntimeDependencies,
        binding: ApprovedPeerBinding,
        bound: BoundProvider,
        settings: ExecutorConfig,
    ) -> Result<Self, AgentRuntimeError> {
        let plan = binding.service_plan().ok_or(AgentRuntimeError::Invalid)?;
        let provider_effects = plan
            .direct_effects()
            .iter()
            .filter(|effect| matches_effect(effect, bound.effect()))
            .count();
        if bound.catalog_version().is_none()
            || provider_effects != 1
            || plan.direct_effects().iter().any(|effect| {
                !matches_effect(effect, bound.effect()) && effect.operation != "agent.invoke"
            })
        {
            return Err(AgentRuntimeError::Invalid);
        }
        let executor = GovernedProviderExecutor::new(
            dependencies.state.clone(),
            dependencies.coordinator.clone(),
            dependencies.contexts.clone(),
            bound.clone(),
            settings,
            dependencies.clock.clone(),
            None,
        )?
        .require_credential_authority();
        let tasks =
            TaskEngine::new(dependencies.state.clone()).with_clock(dependencies.clock.clone());
        Ok(Self {
            dependencies,
            binding,
            bound,
            executor,
            tasks,
        })
    }

    /// Install a host-qualified provider abort adapter and matching finality
    /// verifier before sharing this runtime. The adapter can request an abort;
    /// only the verifier can establish provider finality.
    pub fn with_trusted_provider_abort(
        mut self,
        adapter: Arc<dyn acteon_executor::governed::abort::ProviderAbortAdapter>,
        verifier: Arc<
            dyn acteon_executor::governed::reconciliation::ProviderReconciliationVerifier,
        >,
    ) -> Result<Self, AgentRuntimeError> {
        self.executor = self
            .executor
            .with_trusted_reconciliation_verifier(verifier)?
            .with_trusted_abort_adapter(adapter)?;
        Ok(self)
    }

    /// Fixed message-to-provider mapping, v1. The complete validated message is
    /// data; labels, metadata and text never select authority or the provider binding.
    /// The host must qualify how the bound provider interprets message content.
    /// Authenticate recipient admission against this action's semantic digest.
    pub fn prepare_message(&self, message: &TaskMessage) -> Result<Action, AgentRuntimeError> {
        message.validate().map_err(|_| AgentRuntimeError::Invalid)?;
        if message.task_id.is_some() {
            return Err(AgentRuntimeError::Invalid);
        }
        let value = serde_json::to_value(message)?;
        if serde_json::to_vec(&value)?.len() > MAX_ACCEPTANCE_BYTES / 2 {
            return Err(AgentRuntimeError::Invalid);
        }
        let resource = self.binding.agent_resource();
        Ok(Action::new(
            resource.namespace(),
            resource.tenant(),
            self.bound.provider_name(),
            self.bound.action_type(),
            serde_json::json!({"a2a_message":value}),
        ))
    }

    fn key(&self, id: uuid::Uuid) -> StateKey {
        let resource = self.binding.agent_resource();
        StateKey::new(
            resource.namespace(),
            resource.tenant(),
            KeyKind::Custom(ACCEPTANCE_KIND.into()),
            id.to_string(),
        )
    }

    fn continuation_key(&self, id: uuid::Uuid) -> StateKey {
        let resource = self.binding.agent_resource();
        StateKey::new(
            resource.namespace(),
            resource.tenant(),
            KeyKind::Custom(CONTINUATION_KIND.into()),
            id.to_string(),
        )
    }

    fn continuation_execution_id(task_id: uuid::Uuid, challenge_id: &str) -> uuid::Uuid {
        uuid::Uuid::new_v5(&task_id, challenge_id.as_bytes())
    }

    fn message_digest(message: &TaskMessage) -> Result<String, AgentRuntimeError> {
        fn canonical(value: serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(map) => serde_json::Value::Object(
                    map.into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect(),
                ),
                serde_json::Value::Array(values) => {
                    serde_json::Value::Array(values.into_iter().map(canonical).collect())
                }
                scalar => scalar,
            }
        }
        let value = canonical(serde_json::to_value(message)?);
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
    }

    fn prepare_continuation_action(
        &self,
        task: &Task,
        challenge_id: &str,
        response: &TaskMessage,
    ) -> Result<Action, AgentRuntimeError> {
        if response.role != TaskRole::User
            || response.task_id.as_deref() != Some(task.id.as_str())
            || response.context_id != task.context_id
            || response
                .metadata
                .get(TASK_CHALLENGE_ID_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                != Some(challenge_id)
        {
            return Err(AgentRuntimeError::Invalid);
        }
        response
            .validate_in_task(&task.id)
            .map_err(|_| AgentRuntimeError::Invalid)?;
        if task
            .history
            .iter()
            .any(|message| message.message_id == response.message_id)
        {
            return Err(AgentRuntimeError::Conflict);
        }
        let mut history = task.history.clone();
        history.push(response.clone());
        let payload = serde_json::json!({
            "a2a_message": response,
            "a2a_history": history,
            "a2a_continuation": {
                "taskId": task.id,
                "contextId": task.context_id,
                "challengeId": challenge_id,
            },
        });
        if serde_json::to_vec(&payload)?.len() > MAX_ACCEPTANCE_BYTES / 2 {
            return Err(AgentRuntimeError::Invalid);
        }
        let resource = self.binding.agent_resource();
        Ok(Action::new(
            resource.namespace(),
            resource.tenant(),
            self.bound.provider_name(),
            self.bound.action_type(),
            payload,
        ))
    }

    fn decode_continuation(
        &self,
        raw: &str,
        accepted: &Acceptance,
    ) -> Result<Continuation, AgentRuntimeError> {
        if raw.len() > MAX_ACCEPTANCE_BYTES {
            return Err(AgentRuntimeError::Invalid);
        }
        let continuation: Continuation = serde_json::from_str(raw)?;
        let expected_child =
            Self::continuation_execution_id(continuation.task_id, &continuation.challenge_id);
        let expected_predecessor = continuation
            .prior_task
            .history
            .iter()
            .rev()
            .find_map(|message| {
                (message.role == TaskRole::User)
                    .then(|| {
                        message
                            .metadata
                            .get(TASK_CHALLENGE_ID_METADATA_KEY)
                            .and_then(serde_json::Value::as_str)
                    })
                    .flatten()
            })
            .map_or(accepted.reference.execution_id(), |challenge| {
                Self::continuation_execution_id(continuation.task_id, challenge)
            });
        if continuation.schema != 1
            || continuation.binding_digest != self.binding.digest()
            || continuation.task_id != accepted.reference.execution_id()
            || continuation.prior_task.id != continuation.task_id.to_string()
            || continuation.prior_task.namespace != accepted.reference.namespace()
            || continuation.prior_task.tenant != accepted.reference.tenant()
            || continuation.prior_task.status.state != TaskState::InputRequired
            || continuation.prior_task.pending_approval_id.as_deref()
                != Some(continuation.challenge_id.as_str())
            || continuation.child_execution_id != expected_child
            || continuation.parent.namespace() != accepted.reference.namespace()
            || continuation.parent.tenant() != accepted.reference.tenant()
            || continuation.parent.principal() != accepted.reference.principal()
            || continuation.parent != accepted.reference
            || continuation.predecessor.execution_id() != expected_predecessor
            || continuation.predecessor.namespace() != accepted.reference.namespace()
            || continuation.predecessor.tenant() != accepted.reference.tenant()
            || continuation.predecessor.principal() != accepted.reference.principal()
            || continuation.response_digest != Self::message_digest(&continuation.response)?
            || continuation.permits != accepted.permits
            || continuation.action.provider.as_str() != self.bound.provider_name()
            || continuation.action.action_type != self.bound.action_type()
            || governed_provider_input_digest(&continuation.action)?
                != governed_provider_input_digest(&self.prepare_continuation_action(
                    &continuation.prior_task,
                    &continuation.challenge_id,
                    &continuation.response,
                )?)?
        {
            return Err(AgentRuntimeError::Conflict);
        }
        continuation
            .prior_task
            .validate()
            .map_err(|_| AgentRuntimeError::Invalid)?;
        TaskEngine::validate_governed_projection(&continuation.prior_task, &accepted.initial_task)
            .map_err(TaskEngineError::from)?;
        permit_revision_tag(&continuation.permits)?;
        if let ContinuationState::Admitted { reference } = &continuation.state
            && (reference.execution_id() != continuation.child_execution_id
                || reference.principal() != accepted.reference.principal()
                || reference.namespace() != accepted.reference.namespace()
                || reference.tenant() != accepted.reference.tenant()
                || reference.request_digest()
                    != governed_provider_input_digest(&continuation.action)?)
        {
            return Err(AgentRuntimeError::Conflict);
        }
        Ok(continuation)
    }

    async fn load_continuation_versioned(
        &self,
        accepted: &Acceptance,
    ) -> Result<Option<(Continuation, u64)>, AgentRuntimeError> {
        self.dependencies
            .state
            .get_versioned(&self.continuation_key(accepted.reference.execution_id()))
            .await?
            .map(|(raw, version)| Ok((self.decode_continuation(&raw, accepted)?, version)))
            .transpose()
    }

    fn decode(&self, raw: &str) -> Result<Acceptance, AgentRuntimeError> {
        if raw.len() > MAX_ACCEPTANCE_BYTES {
            return Err(AgentRuntimeError::Invalid);
        }
        let accepted: Acceptance = serde_json::from_str(raw)?;
        if accepted.schema != 1
            || accepted.binding_digest != self.binding.digest()
            || accepted.reference.principal() != self.binding.target()
            || accepted.reference.namespace() != self.binding.agent_resource().namespace()
            || accepted.reference.tenant() != self.binding.agent_resource().tenant()
            || accepted.initial_task.id != accepted.reference.execution_id().to_string()
            || accepted.initial_task.namespace != accepted.reference.namespace()
            || accepted.initial_task.tenant != accepted.reference.tenant()
            || accepted.initial_task.status.state != TaskState::Submitted
            || accepted.initial_task.context_id
                != accepted
                    .initial_task
                    .history
                    .first()
                    .and_then(|m| m.context_id.clone())
            || accepted
                .initial_task
                .metadata
                .get(GOVERNED_TASK_METADATA_KEY)
                != Some(
                    &serde_json::json!({"execution_id": accepted.reference.execution_id(), "binding_digest": self.binding.digest()}),
                )
            || accepted.initial_task.chain_id.is_some()
            || !accepted.initial_task.artifacts.is_empty()
            || accepted.initial_task.history.len() != 1
            || accepted.initial_task.history[0].task_id.as_deref()
                != Some(accepted.initial_task.id.as_str())
            || governed_provider_input_digest(&accepted.action)?
                != accepted.reference.request_digest()
            || accepted.action.provider.as_str() != self.bound.provider_name()
            || accepted.action.action_type != self.bound.action_type()
        {
            return Err(AgentRuntimeError::Conflict);
        }
        accepted
            .initial_task
            .validate()
            .map_err(|_| AgentRuntimeError::Invalid)?;
        permit_revision_tag(&accepted.permits)?;
        let mut message = accepted.initial_task.history[0].clone();
        message.task_id = None;
        if governed_provider_input_digest(&self.prepare_message(&message)?)?
            != accepted.reference.request_digest()
        {
            return Err(AgentRuntimeError::Conflict);
        }
        Ok(accepted)
    }

    async fn verify(&self, accepted: &Acceptance) -> Result<(), AgentRuntimeError> {
        let context = self
            .dependencies
            .contexts
            .recover_reference_for_observation(&accepted.reference)
            .await?;
        if permit_revision_tag(&accepted.permits)? != context.accepted_ceiling_revision() {
            return Err(AgentRuntimeError::Conflict);
        }
        self.dependencies
            .coordinator
            .verify_service_runtime_binding(
                &context,
                self.binding.digest(),
                self.binding
                    .service_plan()
                    .ok_or(AgentRuntimeError::Invalid)?
                    .direct_effects(),
            )
            .await?;
        Ok(())
    }

    /// Persist acceptance before materializing its Task projection. The accepted
    /// recipient UUID is the stable task ID, including after a lost response.
    /// Replays return original work; they do not allocate or invoke a provider.
    pub async fn accept(
        &self,
        context: &VerifiedExecutionContext,
        permits: &[PermitReference],
        message: &TaskMessage,
    ) -> Result<Task, AgentRuntimeError> {
        let action = self.prepare_message(message)?;
        let reference = context.reference()?;
        if reference.principal() != self.binding.target()
            || governed_provider_input_digest(&action)? != reference.request_digest()
        {
            return Err(AgentRuntimeError::Conflict);
        }
        let mut selected = permits.to_vec();
        selected.sort_by(|a, b| a.id.cmp(&b.id));
        let mut task = Task::new_at(
            reference.execution_id().to_string(),
            reference.namespace(),
            reference.tenant(),
            self.dependencies.clock.now(),
        );
        task.context_id.clone_from(&message.context_id);
        task.metadata.insert(GOVERNED_TASK_METADATA_KEY.into(), serde_json::json!({"execution_id":reference.execution_id(), "binding_digest":self.binding.digest()}));
        let mut initial = message.clone();
        initial.task_id = Some(task.id.clone());
        task.append_history_at(initial, self.dependencies.clock.now())
            .map_err(|_| AgentRuntimeError::Invalid)?;
        let proposed = Acceptance {
            schema: 1,
            binding_digest: self.binding.digest().into(),
            reference,
            permits: selected,
            action,
            initial_task: task,
        };
        self.verify(&proposed).await?;
        let key = self.key(proposed.reference.execution_id());
        let existing = self.dependencies.state.get(&key).await?;
        let raw = if let Some(raw) = existing {
            raw
        } else {
            self.dependencies
                .coordinator
                .check_queued_effect_authority(
                    context,
                    permits,
                    self.bound.effect(),
                    self.dependencies.clock.as_ref(),
                )
                .await?;
            let payload = serde_json::to_string(&proposed)?;
            if payload.len() > MAX_ACCEPTANCE_BYTES {
                return Err(AgentRuntimeError::Invalid);
            }
            if self
                .dependencies
                .state
                .check_and_set(&key, &payload, None)
                .await?
            {
                payload
            } else {
                self.dependencies
                    .state
                    .get(&key)
                    .await?
                    .ok_or(AgentRuntimeError::Missing)?
            }
        };
        let original = self.decode(&raw)?;
        if original.reference != proposed.reference
            || original.permits != proposed.permits
            || governed_provider_input_digest(&original.action)?
                != governed_provider_input_digest(&proposed.action)?
        {
            return Err(AgentRuntimeError::Conflict);
        }
        let task = self.materialize(&original).await?;
        if !task.status.state.is_terminal() {
            return Ok(task);
        }
        Ok(self.observe(original.reference.execution_id()).await?.task)
    }

    async fn materialize(&self, accepted: &Acceptance) -> Result<Task, AgentRuntimeError> {
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        match self.tasks.create_task(accepted.initial_task.clone()).await {
            Ok(task) => Ok(task),
            Err(TaskEngineError::AlreadyExists(_)) => {
                let task = self
                    .tasks
                    .get_task(&scope, &accepted.initial_task.id)
                    .await?
                    .ok_or(AgentRuntimeError::Missing)?;
                TaskEngine::validate_governed_projection(&task, &accepted.initial_task)
                    .map_err(TaskEngineError::from)?;
                Ok(task)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Recover the immediate requester from the immutable signed acceptance.
    /// Hosts must authenticate observation before returning any task contents.
    pub async fn source_context(
        &self,
        task_id: uuid::Uuid,
    ) -> Result<VerifiedExecutionContext, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let context = self
            .dependencies
            .contexts
            .recover_reference_for_observation(&accepted.reference)
            .await?;
        context
            .immediate_service_source()
            .ok_or(AgentRuntimeError::Conflict)
    }

    /// Recover the current recipient context for a host-owned tool invocation.
    /// The task ID is an opaque lookup handle, never an authority token.
    pub async fn recipient_context(
        &self,
        task_id: uuid::Uuid,
    ) -> Result<VerifiedExecutionContext, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        self.dependencies
            .contexts
            .recover_reference(
                &accepted.reference,
                self.dependencies.clock.now().timestamp_millis(),
            )
            .await
            .map_err(Into::into)
    }

    async fn load_acceptance(&self, task_id: uuid::Uuid) -> Result<Acceptance, AgentRuntimeError> {
        let raw = self
            .dependencies
            .state
            .get(&self.key(task_id))
            .await?
            .ok_or(AgentRuntimeError::Missing)?;
        let accepted = self.decode(&raw)?;
        if accepted.reference.execution_id() != task_id {
            return Err(AgentRuntimeError::Conflict);
        }
        self.verify(&accepted).await?;
        Ok(accepted)
    }

    async fn continuation_parent(
        &self,
        accepted: &Acceptance,
    ) -> Result<VerifiedExecutionContext, AgentRuntimeError> {
        self.dependencies
            .contexts
            .recover_reference(
                &accepted.reference,
                self.dependencies.clock.now().timestamp_millis(),
            )
            .await
            .map_err(Into::into)
    }

    async fn completed_continuation_predecessor(
        &self,
        accepted: &Acceptance,
        task: &Task,
        previous: Option<&Continuation>,
    ) -> Result<ExecutionContextReference, AgentRuntimeError> {
        let predecessor = if let Some(record) = previous {
            let ContinuationState::Admitted { reference } = &record.state else {
                return Err(AgentRuntimeError::Conflict);
            };
            let Some(applied) = task
                .history
                .iter()
                .find(|message| message.message_id == record.response.message_id)
            else {
                return Err(AgentRuntimeError::Conflict);
            };
            if serde_json::to_value(applied)? != serde_json::to_value(&record.response)? {
                return Err(AgentRuntimeError::Conflict);
            }
            reference
        } else {
            &accepted.reference
        };
        if !self
            .executor
            .inspect(predecessor, self.binding.target())
            .await?
            .is_some_and(|receipt| {
                matches!(receipt.status, GovernedProviderStatus::Completed { .. })
            })
        {
            return Err(AgentRuntimeError::Conflict);
        }
        Ok(predecessor.clone())
    }

    async fn register_continuation(
        &self,
        accepted: &Acceptance,
        task: &Task,
        challenge_id: &str,
        response: &TaskMessage,
    ) -> Result<Continuation, AgentRuntimeError> {
        let key = self.continuation_key(accepted.reference.execution_id());
        for _ in 0..MAX_CONTINUATION_CAS_ATTEMPTS {
            let previous = self.load_continuation_versioned(accepted).await?;
            if let Some((record, _)) = &previous
                && record.challenge_id == challenge_id
            {
                if record.response_digest != Self::message_digest(response)?
                    || serde_json::to_value(&record.response)? != serde_json::to_value(response)?
                {
                    return Err(AgentRuntimeError::Conflict);
                }
                return Ok(record.clone());
            }
            let predecessor = self
                .completed_continuation_predecessor(
                    accepted,
                    task,
                    previous.as_ref().map(|(record, _)| record),
                )
                .await?;
            let parent = self.continuation_parent(accepted).await?;
            let snapshot = self.dependencies.coordinator.snapshot().await?;
            let limits = snapshot
                .roots
                .get(&parent.execution_id().to_string())
                .ok_or(AgentRuntimeError::Conflict)?
                .limits
                .clone();
            let action = self.prepare_continuation_action(task, challenge_id, response)?;
            let proposed = Continuation {
                schema: 1,
                binding_digest: self.binding.digest().into(),
                task_id: accepted.reference.execution_id(),
                challenge_id: challenge_id.into(),
                parent: parent.reference()?,
                predecessor,
                child_handle: ExecutionContextHandle::new(),
                child_execution_id: Self::continuation_execution_id(
                    accepted.reference.execution_id(),
                    challenge_id,
                ),
                limits,
                permits: accepted.permits.clone(),
                prior_task: task.clone(),
                response_digest: Self::message_digest(response)?,
                response: response.clone(),
                action,
                state: ContinuationState::Registered,
            };
            let encoded = serde_json::to_string(&proposed)?;
            if encoded.len() > MAX_ACCEPTANCE_BYTES {
                return Err(AgentRuntimeError::Invalid);
            }
            let stored = match previous {
                None => {
                    self.dependencies
                        .state
                        .check_and_set(&key, &encoded, None)
                        .await?
                }
                Some((_, version)) => matches!(
                    self.dependencies
                        .state
                        .compare_and_swap(&key, version, &encoded, None)
                        .await?,
                    CasResult::Ok
                ),
            };
            if stored {
                return Ok(proposed);
            }
        }
        Err(AgentRuntimeError::Conflict)
    }

    async fn admit_continuation(
        &self,
        accepted: &Acceptance,
        mut continuation: Continuation,
    ) -> Result<Continuation, AgentRuntimeError> {
        if matches!(continuation.state, ContinuationState::Admitted { .. }) {
            return Ok(continuation);
        }
        let parent = self
            .dependencies
            .contexts
            .recover_reference(
                &continuation.parent,
                self.dependencies.clock.now().timestamp_millis(),
            )
            .await?;
        let snapshot = self.dependencies.coordinator.snapshot().await?;
        if snapshot
            .roots
            .get(&parent.execution_id().to_string())
            .map(|root| &root.limits)
            != Some(&continuation.limits)
        {
            return Err(AgentRuntimeError::Conflict);
        }
        let child = self
            .dependencies
            .contexts
            .capture_child(ChildContextAdmission {
                admission_key: &format!(
                    "agent-continuation/{}/{}",
                    continuation.task_id, continuation.challenge_id
                ),
                parent: &parent,
                handle: continuation.child_handle.clone(),
                execution_id: continuation.child_execution_id,
                request_digest: governed_provider_input_digest(&continuation.action)?,
                accepted_effects: vec![self.bound.effect().clone()],
                restrictions: Vec::new(),
                permits: &continuation.permits,
                limits: continuation.limits.clone(),
                clock: self.dependencies.clock.as_ref(),
            })
            .await?;
        let reference = child.reference()?;
        let key = self.continuation_key(accepted.reference.execution_id());
        for _ in 0..MAX_CONTINUATION_CAS_ATTEMPTS {
            let (current, version) = self
                .load_continuation_versioned(accepted)
                .await?
                .ok_or(AgentRuntimeError::Missing)?;
            if current.challenge_id != continuation.challenge_id
                || current.response_digest != continuation.response_digest
            {
                return Err(AgentRuntimeError::Conflict);
            }
            if matches!(current.state, ContinuationState::Admitted { .. }) {
                return Ok(current);
            }
            continuation = current;
            continuation.state = ContinuationState::Admitted {
                reference: reference.clone(),
            };
            let encoded = serde_json::to_string(&continuation)?;
            if matches!(
                self.dependencies
                    .state
                    .compare_and_swap(&key, version, &encoded, None)
                    .await?,
                CasResult::Ok
            ) {
                return Ok(continuation);
            }
        }
        Err(AgentRuntimeError::Conflict)
    }

    async fn recover_continuation(
        &self,
        accepted: &Acceptance,
        mut task: Task,
    ) -> Result<(Task, Option<Continuation>), AgentRuntimeError> {
        let Some((mut continuation, _)) = self.load_continuation_versioned(accepted).await? else {
            return Ok((task, None));
        };
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        if matches!(continuation.state, ContinuationState::Registered) {
            if !self
                .executor
                .inspect(&continuation.predecessor, self.binding.target())
                .await?
                .is_some_and(|receipt| {
                    matches!(receipt.status, GovernedProviderStatus::Completed { .. })
                })
            {
                return Err(AgentRuntimeError::Conflict);
            }
            let recipient = self
                .dependencies
                .contexts
                .recover_reference(
                    &accepted.reference,
                    self.dependencies.clock.now().timestamp_millis(),
                )
                .await?;
            self.dependencies
                .coordinator
                .check_queued_effect_authority(
                    &recipient,
                    &accepted.permits,
                    self.bound.effect(),
                    self.dependencies.clock.as_ref(),
                )
                .await?;
            if self
                .dependencies
                .coordinator
                .snapshot()
                .await?
                .roots
                .get(&recipient.execution_id().to_string())
                .map(|root| &root.limits)
                != Some(&continuation.limits)
            {
                return Err(AgentRuntimeError::Conflict);
            }
            if task.status.state == TaskState::InputRequired
                && task.pending_approval_id.as_deref() != Some(continuation.challenge_id.as_str())
            {
                return Err(AgentRuntimeError::Conflict);
            }
            if !matches!(
                task.status.state,
                TaskState::InputRequired | TaskState::Working
            ) {
                return Err(AgentRuntimeError::Conflict);
            }
            let source = recipient
                .immediate_service_source()
                .ok_or(AgentRuntimeError::Conflict)?
                .principal()
                .id()
                .to_owned();
            task = self
                .tasks
                .resolve_input(
                    &scope,
                    &task.id,
                    &continuation.challenge_id,
                    continuation.response.clone(),
                    source,
                )
                .await?
                .0;
        }
        let applied = task
            .history
            .iter()
            .find(|message| message.message_id == continuation.response.message_id)
            .ok_or(AgentRuntimeError::Conflict)?;
        if task.status.state != TaskState::Working
            || serde_json::to_value(applied)? != serde_json::to_value(&continuation.response)?
        {
            return Err(AgentRuntimeError::Conflict);
        }
        continuation = self.admit_continuation(accepted, continuation).await?;
        Ok((task, Some(continuation)))
    }

    /// Consume one exact `InputRequired` response from the authenticated original
    /// source and run a separately governed provider continuation. Intent is
    /// durable before the task leaves its paused state, so the recovery driver can
    /// finish admission and execution after a lost request or process restart.
    pub async fn continue_input(
        &self,
        task_id: uuid::Uuid,
        challenge_id: &str,
        response: &TaskMessage,
        requester: &VerifiedExecutionContext,
    ) -> Result<AgentTaskReceipt, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let recipient = self
            .dependencies
            .contexts
            .recover_reference_for_observation(&accepted.reference)
            .await?;
        let source = recipient
            .immediate_service_source()
            .ok_or(AgentRuntimeError::Conflict)?;
        if source.reference()? != requester.reference()? {
            return Err(AgentRuntimeError::Conflict);
        }
        let task = self.materialize(&accepted).await?;
        if let Some((existing, _)) = self.load_continuation_versioned(&accepted).await?
            && existing.challenge_id == challenge_id
        {
            if existing.response_digest != Self::message_digest(response)?
                || serde_json::to_value(&existing.response)? != serde_json::to_value(response)?
            {
                return Err(AgentRuntimeError::Conflict);
            }
            return Box::pin(self.resume(task_id)).await;
        }
        if task.status.state != TaskState::InputRequired
            || task.pending_approval_id.as_deref() != Some(challenge_id)
        {
            return Err(AgentRuntimeError::Conflict);
        }
        self.register_continuation(&accepted, &task, challenge_id, response)
            .await?;
        Box::pin(self.resume(task_id)).await
    }

    /// Open an authorization challenge using a complete host-issued policy.
    /// The caller must authenticate the bound recipient before invoking this
    /// method; no verifier or authority fields are accepted from model output.
    pub async fn pause_for_authorization(
        &self,
        task_id: uuid::Uuid,
        requirement: TaskAuthorizationRequirement,
        reason: Option<String>,
        ttl: std::time::Duration,
    ) -> Result<Task, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let recipient = self.recipient_context(task_id).await?;
        if requirement.recipient != *recipient.principal() {
            return Err(AgentRuntimeError::Conflict);
        }
        self.dependencies
            .coordinator
            .recheck_existing_effect_authority(
                &recipient,
                &accepted.permits,
                self.bound.effect(),
                self.dependencies.clock.as_ref(),
            )
            .await?;
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        Ok(self
            .tasks
            .pause_for_authorization(&scope, &task_id.to_string(), requirement, reason, Some(ttl))
            .await?
            .0)
    }

    /// Resolve one exact challenge for the authenticated original requester.
    pub async fn resolve_authorization(
        &self,
        task_id: uuid::Uuid,
        challenge_id: &str,
        requester: &VerifiedExecutionContext,
        verifier: &dyn TaskAuthorizationVerifier,
    ) -> Result<Task, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let recipient = self
            .dependencies
            .contexts
            .recover_reference_for_observation(&accepted.reference)
            .await?;
        let source = recipient
            .immediate_service_source()
            .ok_or(AgentRuntimeError::Conflict)?;
        if source.reference()? != requester.reference()? {
            return Err(AgentRuntimeError::Conflict);
        }
        let current_recipient = self.recipient_context(task_id).await?;
        self.dependencies
            .coordinator
            .recheck_existing_effect_authority(
                &current_recipient,
                &accepted.permits,
                self.bound.effect(),
                self.dependencies.clock.as_ref(),
            )
            .await?;
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        Ok(self
            .tasks
            .resolve_authorization(&scope, &task_id.to_string(), challenge_id, verifier)
            .await?
            .0)
    }

    /// Restrict this accepted recipient subtree using its original signed source.
    /// Trusted hosts must authenticate the current requester before calling this.
    /// This does not settle attempts, refund budgets, release capacity, or assert
    /// that an already delivered provider effect has been cancelled.
    pub async fn stop(
        &self,
        task_id: uuid::Uuid,
        requester: &VerifiedExecutionContext,
    ) -> Result<AgentTaskStopReceipt, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let recipient = self
            .dependencies
            .contexts
            .recover_reference_for_observation(&accepted.reference)
            .await?;
        let source = recipient
            .immediate_service_source()
            .ok_or(AgentRuntimeError::Conflict)?;
        if source.reference()? != requester.reference()? {
            return Err(AgentRuntimeError::Conflict);
        }
        self.dependencies
            .coordinator
            .change(
                &format!("agent-service-stop:{task_id}"),
                AuthorityChange::CancelExecution {
                    execution_id: task_id.to_string(),
                },
                source.principal().id(),
                "original requester stopped future agent-service starts",
            )
            .await?;
        let provider_abort = self
            .executor
            .abort_restricted(&accepted.reference, recipient.principal())
            .await?
            .map(|receipt| receipt.abort);
        let observed = self.observe(task_id).await?;
        Ok(AgentTaskStopReceipt {
            task: observed.task,
            future_starts_blocked: observed.future_starts_blocked,
            provider_abort,
        })
    }

    async fn future_starts_blocked(
        &self,
        accepted: &Acceptance,
    ) -> Result<bool, AgentRuntimeError> {
        let snapshot = self.dependencies.coordinator.snapshot().await?;
        let root = snapshot
            .roots
            .get(&accepted.reference.execution_id().to_string())
            .ok_or(AgentRuntimeError::Conflict)?;
        Ok(root.cancelled)
    }

    /// Read and repair evidence without invoking a provider or reserving capacity.
    /// Current revocation can deny a start without hiding already accepted work.
    pub async fn observe(
        &self,
        task_id: uuid::Uuid,
    ) -> Result<AgentTaskReceipt, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        let task = self.materialize(&accepted).await?;
        let continuation = self.load_continuation_versioned(&accepted).await?;
        let continuation_pending = continuation
            .as_ref()
            .is_some_and(|(record, _)| matches!(record.state, ContinuationState::Registered));
        let current = match continuation.as_ref().map(|(record, _)| record) {
            Some(Continuation {
                state: ContinuationState::Admitted { reference },
                action,
                ..
            }) => Some(CurrentOperation { reference, action }),
            Some(Continuation {
                state: ContinuationState::Registered,
                ..
            }) => None,
            None => Some(CurrentOperation {
                reference: &accepted.reference,
                action: &accepted.action,
            }),
        };
        let execution = match current {
            Some(current) => {
                self.executor
                    .inspect(current.reference, self.binding.target())
                    .await?
            }
            None => None,
        };
        if task.status.state.is_terminal()
            && !execution.as_ref().is_some_and(|receipt| matches!(&receipt.status,
                GovernedProviderStatus::Completed { outcome } if task.status.state == terminal_state(outcome))) {
            return Err(AgentRuntimeError::Conflict);
        }
        match execution {
            Some(execution) => {
                self.project_execution(&scope, &accepted.initial_task, task, execution)
                    .await
            }
            None if task.status.state.is_terminal() => Err(AgentRuntimeError::Conflict),
            None => Ok(AgentTaskReceipt {
                task,
                execution: None,
                future_starts_blocked: self.future_starts_blocked(&accepted).await?,
                continuation_pending,
            }),
        }
    }

    /// Routing hint for a trusted recovery driver. Full qualification is repeated
    /// by observation/resume before any projection or provider operation.
    pub fn acceptance_task_id(&self, raw: &str) -> Result<uuid::Uuid, AgentRuntimeError> {
        let accepted = self.decode(raw)?;
        Ok(accepted.reference.execution_id())
    }

    #[must_use]
    pub fn binding_digest(&self) -> &str {
        self.binding.digest()
    }

    /// Recover original accepted work. Provider execution retains its existing
    /// immutable operation/attempt journal and coordinator start checkpoint.
    #[allow(clippy::too_many_lines)]
    pub async fn resume(&self, task_id: uuid::Uuid) -> Result<AgentTaskReceipt, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        let mut task = self.materialize(&accepted).await?;
        let mut continuation = self
            .load_continuation_versioned(&accepted)
            .await?
            .map(|(record, _)| record);
        if continuation.is_some() && !task.status.state.is_terminal() {
            (task, continuation) = Box::pin(self.recover_continuation(&accepted, task)).await?;
        }
        let current = match continuation.as_ref() {
            Some(Continuation {
                state: ContinuationState::Admitted { reference },
                action,
                ..
            }) => CurrentOperation { reference, action },
            Some(Continuation {
                state: ContinuationState::Registered,
                ..
            }) => return Err(AgentRuntimeError::Conflict),
            None => CurrentOperation {
                reference: &accepted.reference,
                action: &accepted.action,
            },
        };
        let previous = self
            .executor
            .inspect(current.reference, self.binding.target())
            .await?;
        // Terminal/paused Task projections do not authorize additional work.
        if task.status.state != TaskState::Submitted && task.status.state != TaskState::Working {
            if task.status.state.is_terminal()
                && !previous.as_ref().is_some_and(|r| match &r.status {
                    GovernedProviderStatus::Completed { outcome } => {
                        task.status.state == terminal_state(outcome)
                    }
                    _ => false,
                })
            {
                return Err(AgentRuntimeError::Conflict);
            }
            return match previous {
                Some(execution) if task.status.state.is_terminal() => {
                    self.project_execution(&scope, &accepted.initial_task, task, execution)
                        .await
                }
                execution => Ok(AgentTaskReceipt {
                    task,
                    execution,
                    future_starts_blocked: self.future_starts_blocked(&accepted).await?,
                    continuation_pending: false,
                }),
            };
        }
        if task.status.state == TaskState::Submitted {
            task = self
                .tasks
                .start_governed_task(&scope, &accepted.initial_task)
                .await?;
        }
        if task.status.state.is_terminal() {
            let latest = self
                .executor
                .inspect(current.reference, self.binding.target())
                .await?;
            if !latest.as_ref().is_some_and(|r| match &r.status {
                GovernedProviderStatus::Completed { outcome } => {
                    task.status.state == terminal_state(outcome)
                }
                _ => false,
            }) {
                return Err(AgentRuntimeError::Conflict);
            }
            return self
                .project_execution(
                    &scope,
                    &accepted.initial_task,
                    task,
                    latest.ok_or(AgentRuntimeError::Conflict)?,
                )
                .await;
        }
        let execution = match previous {
            Some(receipt)
                if matches!(
                    receipt.status,
                    GovernedProviderStatus::Completed { .. }
                        | GovernedProviderStatus::InFlight { .. }
                        | GovernedProviderStatus::ReconciliationRequired { .. }
                ) =>
            {
                receipt
            }
            _ => {
                self.executor
                    .execute(
                        current.reference,
                        &accepted.permits,
                        current.action,
                        self.binding.target(),
                    )
                    .await?
            }
        };
        self.project_execution(&scope, &accepted.initial_task, task, execution)
            .await
    }

    async fn project_execution(
        &self,
        scope: &TaskScope,
        expected: &Task,
        mut task: Task,
        execution: GovernedProviderReceipt,
    ) -> Result<AgentTaskReceipt, AgentRuntimeError> {
        if let GovernedProviderStatus::Completed { outcome } = &execution.status {
            // A host pause can race with the provider response. The completed
            // predecessor is required evidence for a governed continuation,
            // but it must not erase the active challenge before the exact
            // source response is durably registered.
            if matches!(
                task.status.state,
                TaskState::InputRequired | TaskState::AuthRequired
            ) && task.pending_approval_id.is_some()
            {
                return Ok(AgentTaskReceipt {
                    task,
                    execution: Some(execution),
                    future_starts_blocked: self
                        .dependencies
                        .coordinator
                        .snapshot()
                        .await?
                        .roots
                        .get(&expected.id)
                        .ok_or(AgentRuntimeError::Conflict)?
                        .cancelled,
                    continuation_pending: false,
                });
            }
            let value = serde_json::to_value(outcome)?;
            let inline =
                serde_json::to_vec(&value)?.len() <= acteon_core::bus_task::MAX_PART_DATA_BYTES;
            let result = if inline {
                value
            } else {
                serde_json::json!({"execution_id":expected.id,"result_in_governed_history":true})
            };
            let next = terminal_state(outcome);
            let artifact = Artifact::new("governed-result", vec![TaskPart::data(result)]);
            if task.status.state == next
                && task.artifacts.len() == 1
                && serde_json::to_value(&task.artifacts[0])? == serde_json::to_value(&artifact)?
            {
                return Ok(AgentTaskReceipt {
                    task,
                    execution: Some(execution),
                    future_starts_blocked: self
                        .dependencies
                        .coordinator
                        .snapshot()
                        .await?
                        .roots
                        .get(&expected.id)
                        .ok_or(AgentRuntimeError::Conflict)?
                        .cancelled,
                    continuation_pending: false,
                });
            }
            task = self
                .tasks
                .project_governed_result(scope, expected, next, artifact)
                .await?;
        }
        Ok(AgentTaskReceipt {
            task,
            execution: Some(execution),
            future_starts_blocked: self
                .dependencies
                .coordinator
                .snapshot()
                .await?
                .roots
                .get(&expected.id)
                .ok_or(AgentRuntimeError::Conflict)?
                .cancelled,
            continuation_pending: false,
        })
    }
}

fn terminal_state(outcome: &ActionOutcome) -> TaskState {
    if matches!(outcome, ActionOutcome::Executed(_)) {
        TaskState::Completed
    } else {
        TaskState::Failed
    }
}
