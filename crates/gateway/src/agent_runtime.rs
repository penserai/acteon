//! Durable individual-agent task execution on the existing governed provider boundary.
//!
//! This trusted host adapter accepts an already authenticated, signed recipient
//! context. It is not an A2A codec or permission to send a network request.
use std::sync::Arc;

use acteon_core::{
    Action, ActionOutcome, Artifact, ExecutionContextReference, Task, TaskMessage, TaskPart,
    TaskState,
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
    AuthorityCoordinator, CoordinationError,
    context::{ContextError, TrustedContextStore, VerifiedExecutionContext},
    permit::{PermitReference, matches_effect, permit_revision_tag},
};
use acteon_state::{KeyKind, StateError, StateKey, StateStore};
use acteon_time::Clock;
use serde::{Deserialize, Serialize};

use crate::{TaskEngine, TaskEngineError, TaskScope};

pub const ACCEPTANCE_KIND: &str = "governed_agent_task_acceptance";
pub const GOVERNED_TASK_METADATA_KEY: &str = "acteon_governed_execution";
const MAX_ACCEPTANCE_BYTES: usize = 2 * 1024 * 1024;

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
}

impl AgentProviderRuntime {
    pub fn new_trusted(
        dependencies: AgentRuntimeDependencies,
        binding: ApprovedPeerBinding,
        bound: BoundProvider,
        settings: ExecutorConfig,
    ) -> Result<Self, AgentRuntimeError> {
        let plan = binding.service_plan().ok_or(AgentRuntimeError::Invalid)?;
        if bound.catalog_version().is_none()
            || plan.direct_effects().len() != 1
            || !matches_effect(&plan.direct_effects()[0], bound.effect())
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
                std::slice::from_ref(self.bound.effect()),
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
        let execution = self
            .executor
            .inspect(&original.reference, self.binding.target())
            .await?
            .ok_or(AgentRuntimeError::Conflict)?;
        if !matches!(&execution.status, GovernedProviderStatus::Completed { outcome } if task.status.state == terminal_state(outcome))
        {
            return Err(AgentRuntimeError::Conflict);
        }
        let scope = TaskScope::new(original.reference.namespace(), original.reference.tenant());
        Ok(self
            .project_execution(&scope, &original.initial_task, task, execution)
            .await?
            .task)
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

    /// Read and repair evidence without invoking a provider or reserving capacity.
    /// Current revocation can deny a start without hiding already accepted work.
    pub async fn observe(
        &self,
        task_id: uuid::Uuid,
    ) -> Result<AgentTaskReceipt, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        let task = self.materialize(&accepted).await?;
        let execution = self
            .executor
            .inspect(&accepted.reference, self.binding.target())
            .await?;
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
    pub async fn resume(&self, task_id: uuid::Uuid) -> Result<AgentTaskReceipt, AgentRuntimeError> {
        let accepted = self.load_acceptance(task_id).await?;
        let scope = TaskScope::new(accepted.reference.namespace(), accepted.reference.tenant());
        let mut task = self.materialize(&accepted).await?;
        let previous = self
            .executor
            .inspect(&accepted.reference, self.binding.target())
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
                execution => Ok(AgentTaskReceipt { task, execution }),
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
                .inspect(&accepted.reference, self.binding.target())
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
                        &accepted.reference,
                        &accepted.permits,
                        &accepted.action,
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
