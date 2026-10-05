//! Public workforce declarations. None of these DTOs establishes caller authority.
use super::{
    AgentOwnership, RepresentedParty, TeamRef, WorkforceAssignment, WorkforceDependency,
    WorkforceMembership, WorkforceReference, WorkforceTeam,
};
use crate::{
    GovernanceEffect, GovernanceLimits, GovernancePermitDeclaration, GovernanceRoute,
    GovernanceRouteView, PrincipalIdentity,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceMandateDeclaration {
    pub id: String,
    pub revision: u64,
    pub represented: RepresentedParty,
    pub actor: PrincipalIdentity,
    pub job_class: String,
    pub eligible_initiators: Vec<PrincipalIdentity>,
    pub ownership: Option<WorkforceReference>,
    pub dependencies: Vec<WorkforceDependency>,
    pub routes: Vec<GovernanceRoute>,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkforceChange {
    PutTeam {
        team: WorkforceTeam,
    },
    DisbandTeam {
        team: TeamRef,
        expected_revision: u64,
    },
    PutMembership {
        membership: WorkforceMembership,
    },
    RemoveMembership {
        id: String,
        expected_revision: u64,
    },
    PutOwnership {
        ownership: AgentOwnership,
    },
    PutAssignment {
        assignment: WorkforceAssignment,
    },
    RemoveAssignment {
        id: String,
        expected_revision: u64,
    },
    PutMandate {
        mandate: WorkforceMandateDeclaration,
    },
    RevokeMandate {
        id: String,
        expected_revision: u64,
    },
    PublishRepresentedPermit {
        permit: GovernancePermitDeclaration,
        mandate: WorkforceReference,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceChangeRequest {
    pub namespace: String,
    pub tenant: String,
    pub change_id: String,
    pub change: WorkforceChange,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceEntry<T> {
    pub value: T,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceMandateView {
    pub id: String,
    pub revision: u64,
    pub represented: RepresentedParty,
    pub actor: PrincipalIdentity,
    pub job_class: String,
    pub eligible_initiators: Vec<PrincipalIdentity>,
    pub ownership: Option<WorkforceReference>,
    pub dependencies: Vec<WorkforceDependency>,
    pub effects: Vec<GovernanceEffect>,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforcePermitBindingView {
    pub permit_id: String,
    pub permit_revision: u64,
    pub mandate: WorkforceReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceManagementBounds {
    pub teams: Vec<TeamRef>,
    pub principals: Vec<PrincipalIdentity>,
    pub job_classes: Vec<String>,
    pub can_manage_roster: bool,
    pub can_issue_mandates: bool,
    pub can_issue_permits: bool,
    pub valid_from_ms: i64,
    pub limits: GovernanceLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceScopeView {
    pub namespace: String,
    pub tenant: String,
    pub incarnation: String,
    pub generation: u64,
    pub management: WorkforceManagementBounds,
    pub routes: Vec<GovernanceRouteView>,
    pub teams: Vec<WorkforceEntry<WorkforceTeam>>,
    pub memberships: Vec<WorkforceEntry<WorkforceMembership>>,
    pub ownership: Vec<WorkforceEntry<AgentOwnership>>,
    pub assignments: Vec<WorkforceEntry<WorkforceAssignment>>,
    pub mandates: Vec<WorkforceEntry<WorkforceMandateView>>,
    pub permit_bindings: Vec<WorkforcePermitBindingView>,
}
