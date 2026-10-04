//! Trusted admission and durable settlement at the actual provider boundary.
//!
//! Gates are infrastructure capabilities, not values constructed from request
//! metadata. Implementations authenticate provenance, resolve the complete
//! resources, evaluate current permits and register a unique durable attempt.
use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use async_trait::async_trait;

/// The selected provider can differ from `action.provider` during fallback.
pub struct ProviderAttempt<'a> {
    pub action: &'a Action,
    pub provider_name: &'a str,
    /// Zero-based retry ordinal; this is not a globally unique attempt ID.
    pub ordinal: u32,
    /// Executor clock sampled after semaphore/backoff waits. A gate that waits
    /// must refresh its own trusted time before durable registration.
    pub now_ms: i64,
}

/// Provider evidence, supplied before the executor decides whether to retry.
pub enum ProviderAttemptOutcome<'a> {
    Succeeded(&'a ProviderResponse),
    Failed(&'a ProviderError),
    TimedOut,
}

/// A timeout or connection error alone cannot establish known settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptSettlement {
    /// Durable evidence closes this attempt. Normal retry policy may proceed.
    Settled,
    /// Capacity remains reserved; automatic retries must stop for reconciliation.
    Uncertain,
}

/// Stable, redacted gate failures. These never become retryable provider errors.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum AttemptGateError {
    #[error("current execution authority denies this attempt")]
    Denied,
    #[error("execution authority coordination is unavailable")]
    Unavailable,
    #[error("execution attempt conflicts with its durable identity")]
    Conflict,
}

impl AttemptGateError {
    /// Safe machine-readable admission reason, independent of adapter details.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Denied => "ATTEMPT_DENIED",
            Self::Unavailable => "ATTEMPT_AUTHORITY_UNAVAILABLE",
            Self::Conflict => "ATTEMPT_CONFLICT",
        }
    }
}

/// A newly registered attempt, never an observed/replayed registration.
///
/// Dropping the executor future cannot invoke async settlement. Implementations
/// must leave their durable registration unresolved on cancellation/panic; a
/// lease expiry or Drop must not release capacity or authorize another send.
#[async_trait]
pub trait RegisteredProviderAttempt: Send {
    async fn finish(
        self: Box<Self>,
        outcome: ProviderAttemptOutcome<'_>,
    ) -> Result<AttemptSettlement, AttemptGateError>;
}

/// Trusted host adapter. Each start must freshly evaluate current authority and
/// atomically register the complete effect and its root reservation. Returning
/// an existing registration as a new guard would incorrectly authorize a send.
#[async_trait]
pub trait ProviderAttemptGate: Send + Sync {
    async fn start(
        &self,
        attempt: ProviderAttempt<'_>,
    ) -> Result<Box<dyn RegisteredProviderAttempt>, AttemptGateError>;
}
