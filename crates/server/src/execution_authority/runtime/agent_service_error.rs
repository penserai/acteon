//! Public categories never carry private credentials, backend details or task data.
use acteon_executor::governed::GovernedProviderError;
use acteon_gateway::{agent_runtime::AgentRuntimeError, task_engine::TaskEngineError};
use acteon_governance::{CoordinationError, context::ContextError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentServiceError {
    #[error("invalid agent service request")]
    Invalid,
    #[error("current agent service authority is required")]
    Forbidden,
    #[error("agent service task unavailable to this requester")]
    NotFound,
    #[error("agent service work conflicts with its accepted definition")]
    Conflict,
    #[error("agent service capacity or budget exhausted")]
    Limits,
    #[error("agent service state or runtime unavailable")]
    Unavailable,
}
impl AgentServiceError {
    pub(super) fn authentication(error: &CoordinationError) -> Self {
        match error {
            CoordinationError::State(_)
            | CoordinationError::Invalid(_)
            | CoordinationError::Contention => Self::Unavailable,
            _ => Self::Forbidden,
        }
    }

    /// Conceal ownership and missing provenance on requester reads. Storage
    /// failures remain unavailable rather than claiming the task does not exist.
    pub(super) fn observation(error: AgentRuntimeError) -> Self {
        match Self::from(error) {
            Self::Unavailable => Self::Unavailable,
            _ => Self::NotFound,
        }
    }
}
impl From<CoordinationError> for AgentServiceError {
    fn from(error: CoordinationError) -> Self {
        match error {
            CoordinationError::Conflict | CoordinationError::StaleAuthority => Self::Conflict,
            CoordinationError::Restricted
            | CoordinationError::PermitDenied(_)
            | CoordinationError::DeadlineExceeded => Self::Forbidden,
            CoordinationError::Capacity
            | CoordinationError::BudgetExhausted
            | CoordinationError::ConcurrencyExhausted => Self::Limits,
            CoordinationError::State(_)
            | CoordinationError::Invalid(_)
            | CoordinationError::Contention => Self::Unavailable,
        }
    }
}
impl From<ContextError> for AgentServiceError {
    fn from(error: ContextError) -> Self {
        match error {
            ContextError::State(_) => Self::Unavailable,
            ContextError::Coordination(error) => error.into(),
            ContextError::Conflict => Self::Conflict,
            ContextError::Missing => Self::NotFound,
            ContextError::Invalid => Self::Invalid,
            ContextError::Verification | ContextError::Expired | ContextError::Incarnation => {
                Self::Forbidden
            }
        }
    }
}
impl From<AgentRuntimeError> for AgentServiceError {
    fn from(error: AgentRuntimeError) -> Self {
        match error {
            AgentRuntimeError::Invalid => Self::Invalid,
            AgentRuntimeError::Conflict => Self::Conflict,
            AgentRuntimeError::Missing => Self::NotFound,
            AgentRuntimeError::State(_) | AgentRuntimeError::Encoding(_) => Self::Unavailable,
            AgentRuntimeError::Context(error) => error.into(),
            AgentRuntimeError::Authority(error) => error.into(),
            AgentRuntimeError::Execution(error) => match error {
                GovernedProviderError::Invalid => Self::Invalid,
                GovernedProviderError::Ownership | GovernedProviderError::Admission(_) => {
                    Self::Forbidden
                }
                GovernedProviderError::Conflict => Self::Conflict,
                GovernedProviderError::Unavailable => Self::Unavailable,
            },
            AgentRuntimeError::Task(error) => match error {
                TaskEngineError::State(_)
                | TaskEngineError::Audit(_)
                | TaskEngineError::Serde(_)
                | TaskEngineError::CasExhausted(_) => Self::Unavailable,
                TaskEngineError::NotFound(_) => Self::NotFound,
                _ => Self::Conflict,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_state::StateError;

    #[test]
    fn backend_failures_remain_unavailable_through_nested_observation() {
        let failures = [
            AgentRuntimeError::State(StateError::Connection("private backend address".into())),
            AgentRuntimeError::Context(ContextError::State(StateError::Backend(
                "private context".into(),
            ))),
            AgentRuntimeError::Authority(CoordinationError::State(StateError::Timeout(
                std::time::Duration::from_secs(1),
            ))),
            AgentRuntimeError::Task(TaskEngineError::State(StateError::Backend(
                "private task".into(),
            ))),
            AgentRuntimeError::Execution(GovernedProviderError::Unavailable),
        ];
        for error in failures {
            assert_eq!(
                AgentServiceError::observation(error),
                AgentServiceError::Unavailable
            );
        }
    }

    #[test]
    fn observation_conceals_foreign_missing_or_conflicting_provenance() {
        for error in [
            AgentRuntimeError::Missing,
            AgentRuntimeError::Conflict,
            AgentRuntimeError::Context(ContextError::Verification),
            AgentRuntimeError::Execution(GovernedProviderError::Ownership),
        ] {
            assert_eq!(
                AgentServiceError::observation(error),
                AgentServiceError::NotFound
            );
        }
        assert_eq!(
            AgentServiceError::from(ContextError::Conflict),
            AgentServiceError::Conflict
        );
        assert_eq!(
            AgentServiceError::from(CoordinationError::ConcurrencyExhausted),
            AgentServiceError::Limits
        );
        assert_eq!(
            AgentServiceError::from(CoordinationError::DeadlineExceeded),
            AgentServiceError::Forbidden
        );
    }
}
