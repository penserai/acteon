//! Public, read-only projections of authenticated provider execution evidence.
use crate::{ActionOutcome, GovernanceEffect, PrincipalIdentity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderExecutionHistory {
    pub subject: PrincipalIdentity,
    pub receipt: ProviderHistoryReceipt,
    pub observed_authority: ProviderHistoryAuthority,
    pub operation_integrity: ProviderOperationIntegrity,
    pub metadata: Option<ProviderOperationMetadata>,
    pub binding: Option<ProviderHistoryBinding>,
    pub cancellation_fenced: bool,
    pub attempts: Vec<ProviderHistoryAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderHistoryReceipt {
    pub execution_id: String,
    pub attempts: u32,
    pub status: ProviderHistoryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderHistoryStatus {
    Prepared,
    InFlight { attempt_id: String },
    AwaitingRetry { not_before_ms: i64 },
    Completed { outcome: ActionOutcome },
    ReconciliationRequired { attempt_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderHistoryAuthority {
    pub incarnation: String,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProviderOperationIntegrity {
    Unstarted,
    Sealed,
    Legacy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderOperationMetadata {
    pub original_action_id: String,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderHistoryBinding {
    pub provider: String,
    pub provider_revision: String,
    pub failure_revision: String,
    pub effect: GovernanceEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptStatus {
    InFlight,
    Settled,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderEvidenceReference {
    pub id: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderHistoryAttempt {
    pub attempt_id: String,
    pub ordinal: u32,
    pub ledger_status: ProviderAttemptStatus,
    pub original_evidence: Option<ProviderEvidenceReference>,
    pub original_outcome: Option<ActionOutcome>,
    pub reconciliation: Option<ProviderHistoryReconciliation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderReconciliationAcceptance {
    pub operator: PrincipalIdentity,
    pub authority: ProviderHistoryAuthority,
    pub accepted_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderHistoryReconciliation {
    pub prior_status: ProviderAttemptStatus,
    pub execution_id: String,
    pub attempt_id: String,
    pub original_evidence: Option<ProviderEvidenceReference>,
    pub resolution: ProviderEvidenceReference,
    pub verifier_revision: String,
    pub proof_digest: String,
    pub resolved_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<ProviderReconciliationAcceptance>,
    pub outcome: ActionOutcome,
}

#[cfg(test)]
mod tests {
    use super::ProviderExecutionHistory;

    #[test]
    fn sdk_history_fixtures_match_public_wire() {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/provider-history.json"
        ))
        .unwrap();
        for fixture in fixtures {
            let history: ProviderExecutionHistory =
                serde_json::from_value(fixture.clone()).unwrap();
            assert_eq!(serde_json::to_value(history).unwrap(), fixture);
        }
    }
}

/// Correlation metadata for an independently qualified finality source. This
/// observation never grants permission to dispatch, retry or settle an attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderReconciliationCorrelation {
    pub context: crate::ExecutionContextReference,
    pub action_id: String,
    pub attempt_id: String,
    pub ordinal: u32,
    pub token: String,
    pub binding_digest: String,
}

/// Opaque evidence from the qualified source; verifier selection and actor
/// identity always come from trusted server installation and authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ProviderReconciliationRequest {
    pub proof_base64: String,
}
