//! Public management DTOs. None of these values establish caller authority.
use crate::{PrincipalIdentity, ResourceRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceLimits {
    pub max_units: u64,
    pub max_concurrent: u64,
    pub deadline_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceRoute {
    pub provider: String,
    pub action_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceEffect {
    pub operation: String,
    pub resources: Vec<ResourceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernancePermitDeclaration {
    pub id: String,
    pub revision: u64,
    pub subject: PrincipalIdentity,
    pub routes: Vec<GovernanceRoute>,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
}

/// One exact locally reviewed cross-domain authority shape. This public DTO is
/// configuration and management data; the governance coordinator remains the
/// authority boundary that validates and retains it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FederationGrantCeilingDeclaration {
    pub source: PrincipalIdentity,
    pub target: PrincipalIdentity,
    pub agent_resource: ResourceRef,
    pub binding_digest: String,
    pub skill: String,
    pub ingress_effect: GovernanceEffect,
    pub effects: Vec<GovernanceEffect>,
    pub limits: GovernanceLimits,
    pub max_depth: usize,
}

/// A versioned local trust decision for one foreign signing key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FederationTrustDeclaration {
    pub id: String,
    pub revision: u64,
    pub foreign_domain: String,
    pub local_audience: String,
    pub key_id: String,
    pub verifying_key: String,
    pub minimum_issuer_epoch: u64,
    pub max_revocation_staleness_ms: i64,
    pub max_clock_skew_ms: i64,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
    pub approved: Vec<FederationGrantCeilingDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PublishFederationTrustRequest {
    pub namespace: String,
    pub tenant: String,
    pub change_id: String,
    pub expected_revision: u64,
    pub trust: FederationTrustDeclaration,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct RevokeFederationTrustRequest {
    pub namespace: String,
    pub tenant: String,
    pub change_id: String,
    pub trust_id: String,
    pub expected_revision: u64,
    pub reason: String,
}

/// Exact foreign bytes and signature. The server verifies the payload without
/// reparsing and reserializing it before signature verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SignedFederationEnvelopeDeclaration {
    pub payload: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportFederationEnvelopeRequest {
    pub envelope: SignedFederationEnvelopeDeclaration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SignedFederationInvocationDeclaration {
    pub payload: String,
    pub signature: String,
}

/// Exact signed foreign intent plus the same typed message carried inside that
/// signed payload. The receiver compares them before deriving local provider input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct InvokeFederatedAgentRequest {
    pub invocation: SignedFederationInvocationDeclaration,
    pub message: crate::TaskMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FederationImportReceipt {
    pub namespace: String,
    pub tenant: String,
    pub import_id: String,
    pub envelope_digest: String,
    pub trust_id: String,
    pub trust_revision: u64,
    pub issuer_domain: String,
    pub source: PrincipalIdentity,
    pub target: PrincipalIdentity,
    pub agent_resource: ResourceRef,
    pub binding_digest: String,
    pub skill: String,
    pub root_id: String,
    pub expires_at_ms: i64,
    pub accepted_at_ms: i64,
    /// Import admission does not start an effect or dispatch an agent task.
    pub execution_started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PublishGovernancePermitRequest {
    pub namespace: String,
    pub tenant: String,
    pub change_id: String,
    pub expected_revision: u64,
    pub permit: GovernancePermitDeclaration,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GovernanceIntervention {
    CloseResource {
        resource: ResourceRef,
    },
    ReopenResource {
        resource: ResourceRef,
    },
    RevokeSubject {
        subject: PrincipalIdentity,
    },
    RevokePermit {
        permit_id: String,
        expected_revision: u64,
    },
    RevokeCredential {
        credential_id: String,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceInterventionRequest {
    pub namespace: String,
    pub tenant: String,
    pub change_id: String,
    pub change: GovernanceIntervention,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceChangeReceipt {
    pub namespace: String,
    pub tenant: String,
    pub change_id: String,
    pub actor: String,
    pub reason: String,
    pub generation: u64,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceRouteView {
    pub route: GovernanceRoute,
    pub effect: GovernanceEffect,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernancePermitView {
    pub id: String,
    pub revision: u64,
    pub subject: PrincipalIdentity,
    pub effects: Vec<GovernanceEffect>,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
// Independent additive permissions retain the established wire/config contract.
#[allow(clippy::struct_excessive_bools)]
pub struct GovernanceManagementBounds {
    pub subjects: Vec<PrincipalIdentity>,
    pub can_issue_permits: bool,
    pub can_intervene: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_read_history: bool,
    /// Independent permission to accept qualified finality for retained work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_reconcile: bool,
    /// Independent right to publish or terminally revoke configured federation trust roots.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_manage_federation: bool,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct FederationTrustView {
    pub trust: FederationTrustDeclaration,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceScopeView {
    pub management: GovernanceManagementBounds,
    pub namespace: String,
    pub tenant: String,
    pub incarnation: String,
    pub generation: u64,
    pub routes: Vec<GovernanceRouteView>,
    pub permits: Vec<GovernancePermitView>,
    pub closed_resources: Vec<ResourceRef>,
    pub revoked_subjects: Vec<String>,
    /// Present only for a manager with an independently configured federation footprint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub federation_trusts: Vec<FederationTrustView>,
}

#[cfg(test)]
mod tests {
    use super::GovernanceScopeView;

    #[test]
    fn reconciliation_capability_is_independent_and_preserves_legacy_wire() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/governance-management.json"
        ))
        .unwrap();
        let legacy: GovernanceScopeView = serde_json::from_value(fixture["scope"].clone()).unwrap();
        assert!(!legacy.management.can_reconcile);
        assert!(!legacy.management.can_read_history);
        assert_eq!(serde_json::to_value(legacy).unwrap(), fixture["scope"]);
        let mut permitted = fixture["scope"].clone();
        permitted["management"]["can_reconcile"] = true.into();
        let view: GovernanceScopeView = serde_json::from_value(permitted.clone()).unwrap();
        assert!(view.management.can_reconcile);
        assert!(!view.management.can_read_history);
        assert_eq!(serde_json::to_value(view).unwrap(), permitted);
    }
}

/// One fixed metadata projection. The selector never grants control authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum GovernanceRegistryProjection {
    Agent,
    Card,
}

/// Stable operator intent. Retain this exact request when an acknowledgement is lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceRegistryMutationRequest {
    pub namespace: String,
    pub tenant: String,
    pub agent_id: String,
    pub change_id: String,
    pub expected_registry_revision: u64,
    pub projection: GovernanceRegistryProjection,
    pub expected_projection_version: Option<u64>,
    /// A complete validated Agent/AgentCard. Null requests deletion.
    pub value: Option<serde_json::Value>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceRegistryProjectionView {
    pub namespace: String,
    pub tenant: String,
    pub agent_id: String,
    pub agent_resource: ResourceRef,
    pub projection: GovernanceRegistryProjection,
    pub registry_revision: u64,
    pub qualification_retired: Option<bool>,
    pub version: Option<u64>,
    pub value: Option<serde_json::Value>,
}

/// Known applied metadata, independent of any later agent requalification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GovernanceRegistryMutationReceipt {
    pub namespace: String,
    pub tenant: String,
    pub agent_id: String,
    pub change_id: String,
    pub projection: GovernanceRegistryProjection,
    pub expected_registry_revision: u64,
    pub input_digest: String,
    pub actor: String,
    pub delivery_complete: bool,
    pub applied: bool,
}
