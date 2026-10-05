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
pub struct GovernanceManagementBounds {
    pub subjects: Vec<PrincipalIdentity>,
    pub can_issue_permits: bool,
    pub can_intervene: bool,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
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
}
