//! Trusted authorization verification for `AuthRequired` A2A tasks.
//!
//! Verifiers are installed by the host. They receive an exact, immutable
//! binding assembled from trusted task and approval state; message content and
//! model output cannot select the verifier, recipient, credential authority,
//! audience, scopes, task, or challenge.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use acteon_core::{PrincipalIdentity, TaskAuthorizationRequirement};

use crate::task_engine::TaskScope;

/// Exact verification request assembled from state owned by Acteon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAuthorizationVerification {
    pub scope: TaskScope,
    pub task_id: String,
    pub challenge_id: String,
    pub requirement: TaskAuthorizationRequirement,
}

/// A successful current verifier decision. The decision ID must be stable for
/// one authorization request so recovery can distinguish it from replacement
/// or replayed authorization state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTaskAuthorization {
    pub decision_id: String,
    pub subject: PrincipalIdentity,
    pub verified_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
}

/// Verification failures deliberately distinguish denial from temporary
/// unavailability. Neither outcome mutates the task or approval.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskAuthorizationVerificationError {
    #[error("authorization request is not currently authorized")]
    Denied,
    #[error("authorization verifier is temporarily unavailable")]
    Unavailable,
    #[error("authorization verifier returned invalid evidence: {0}")]
    Invalid(String),
}

/// Host-installed trust adapter. Implementations may introspect an OAuth
/// grant, a workload identity, a vault lease, or another external authority.
/// They must never return success from untrusted task/message content alone.
#[async_trait]
pub trait TaskAuthorizationVerifier: Send + Sync {
    fn verifier_id(&self) -> &str;
    fn revision(&self) -> u64;

    async fn verify(
        &self,
        request: &TaskAuthorizationVerification,
    ) -> Result<VerifiedTaskAuthorization, TaskAuthorizationVerificationError>;
}
