//! Workforce identity and organization. Relationships do not grant tool authority.
use crate::{PrincipalIdentity, ResourceKind, ResourceRef};
use serde::{Deserialize, Serialize};
mod management;
pub use management::*;

/// Teams are organizational identities, not authenticated principals or tenants.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(try_from = "TeamWire")]
pub struct TeamRef {
    domain: String,
    tenant: String,
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamWire {
    domain: String,
    tenant: String,
    id: String,
}
impl TryFrom<TeamWire> for TeamRef {
    type Error = crate::ResourceRefError;
    fn try_from(wire: TeamWire) -> Result<Self, Self::Error> {
        Self::new(wire.domain, wire.tenant, wire.id)
    }
}
impl TeamRef {
    pub fn new(
        domain: impl Into<String>,
        tenant: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, crate::ResourceRefError> {
        let checked = ResourceRef::new(ResourceKind::Agent, domain, tenant, id)?;
        Ok(Self {
            domain: checked.namespace().into(),
            tenant: checked.tenant().into(),
            id: checked.id().into(),
        })
    }
    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Public descriptive identity. Current mandate evaluation establishes representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepresentedParty {
    Human { principal: PrincipalIdentity },
    Team { team: TeamRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceReference {
    pub id: String,
    pub accepted_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkforceDependency {
    Membership { reference: WorkforceReference },
    Assignment { reference: WorkforceReference },
}

/// Team roles confer only the specified workforce capability, never tool grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Requester,
    Approver,
    WorkforceManager,
    MandateIssuer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceTeam {
    pub team: TeamRef,
    pub revision: u64,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceMembership {
    pub id: String,
    pub revision: u64,
    pub team: TeamRef,
    pub human: PrincipalIdentity,
    pub roles: Vec<TeamRole>,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct AgentOwnership {
    pub agent: PrincipalIdentity,
    pub revision: u64,
    pub owner: RepresentedParty,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkforceAssignment {
    pub id: String,
    pub revision: u64,
    pub team: TeamRef,
    pub agent: PrincipalIdentity,
    /// Exact reviewed job classes; assignment is availability, not a permit.
    pub job_classes: Vec<String>,
    pub valid_from_ms: i64,
    pub deadline_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scoped_team_identity_is_validated_without_creating_a_principal() {
        let team = TeamRef::new("workforce", "acme", "reliability").unwrap();
        assert_eq!(
            serde_json::from_value::<TeamRef>(serde_json::to_value(&team).unwrap()).unwrap(),
            team
        );
        for bad in ["", "*", " leading", "trailing ", "control\n"] {
            assert!(TeamRef::new("workforce", "acme", bad).is_err());
        }
        assert!(serde_json::from_value::<TeamRef>(serde_json::json!({"domain":"workforce","tenant":"acme","id":"reliability","principal":"admin"})).is_err());
        assert!(
            serde_json::from_value::<PrincipalIdentity>(
                serde_json::json!({"id":"reliability","kind":"team"})
            )
            .is_err()
        );
        assert_ne!(
            team,
            TeamRef::new("workforce", "other", "reliability").unwrap()
        );
    }
}
