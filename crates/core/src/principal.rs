//! Stable actor identity independent of credential names and secrets.
//!
//! These serializable values are provenance, not execution authority. Only a
//! trusted authentication adapter may bind a presented credential to one.
use serde::{Deserialize, Serialize};

/// Descriptive actor category; it does not confer privileges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Agent,
    Service,
    System,
}

/// Operator-assigned identity within one administrative domain.
/// Namespace/tenant authority is still independently scoped by grants/permits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(try_from = "PrincipalWire")]
pub struct PrincipalIdentity {
    id: String,
    kind: PrincipalKind,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalWire {
    id: String,
    kind: PrincipalKind,
}
impl TryFrom<PrincipalWire> for PrincipalIdentity {
    type Error = PrincipalIdentityError;
    fn try_from(wire: PrincipalWire) -> Result<Self, Self::Error> {
        Self::new(wire.id, wire.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "principal ID must be nonempty, at most 255 UTF-8 bytes, and contain no controls, boundary whitespace, or wildcard-only value"
)]
pub struct PrincipalIdentityError;

impl PrincipalIdentity {
    pub fn new(id: impl Into<String>, kind: PrincipalKind) -> Result<Self, PrincipalIdentityError> {
        let id = id.into();
        if id.is_empty()
            || id.len() > 255
            || id == "*"
            || id.trim() != id
            || id.chars().any(char::is_control)
        {
            return Err(PrincipalIdentityError);
        }
        Ok(Self { id, kind })
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[must_use]
    pub const fn kind(&self) -> PrincipalKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_identity_is_validated_and_is_not_an_authority_envelope() {
        let actor = PrincipalIdentity::new("diagnostic-agent", PrincipalKind::Agent).unwrap();
        assert_eq!(
            serde_json::from_value::<PrincipalIdentity>(serde_json::to_value(&actor).unwrap())
                .unwrap(),
            actor
        );
        for id in ["", "*", " leading", "trailing ", "bad\nvalue"] {
            assert!(
                serde_json::from_value::<PrincipalIdentity>(
                    serde_json::json!({"id":id,"kind":"agent"})
                )
                .is_err()
            );
        }
        assert!(PrincipalIdentity::new("雪".repeat(86), PrincipalKind::Human).is_err());
        assert!(
            serde_json::from_value::<PrincipalIdentity>(
                serde_json::json!({"id":"actor","kind":"agent","permits":["admin"]})
            )
            .is_err()
        );
    }
}

/// Current credential and its optional stable actor binding. This inspection
/// response exposes no credential secret and is not an authority envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CredentialIdentity {
    pub credential_id: String,
    pub auth_method: String,
    pub role: String,
    pub principal: Option<PrincipalIdentity>,
}
