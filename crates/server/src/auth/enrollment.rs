//! Logical credential bindings resolved by authentication, never caller metadata.
use std::collections::BTreeMap;

use acteon_core::PrincipalIdentity;
use serde_json::Value;

use super::authority::canonical_grants;
use super::config::AuthFileConfig;
use super::identity::CallerIdentity;
use super::role::Role;

/// Host-created binding to the credential actually authenticated. It carries no
/// secret and grants no execution authority. Private construction and no
/// Deserialize implementation prevent labels from impersonating enrollment.
#[derive(Debug, Clone)]
pub struct AuthenticatedCredential {
    id: String,
    principal: PrincipalIdentity,
    auth_method: String,
}
impl AuthenticatedCredential {
    pub(super) fn from_identity(
        id: Option<&str>,
        identity: &CallerIdentity,
    ) -> Result<Option<Self>, String> {
        let Some(id) = id else {
            return Ok(None);
        };
        if !valid_id(id) {
            return Err("invalid credential authority ID".into());
        }
        let principal = identity
            .principal
            .as_ref()
            .ok_or("credential authority ID requires a stable principal")?;
        Ok(Some(Self {
            id: id.into(),
            principal: principal.clone(),
            auth_method: identity.auth_method.clone(),
        }))
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[must_use]
    pub fn principal(&self) -> &PrincipalIdentity {
        &self.principal
    }
    #[must_use]
    pub fn auth_method(&self) -> &str {
        &self.auth_method
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id != "*"
        && id.trim() == id
        && !id.chars().any(char::is_control)
}
#[derive(PartialEq)]
struct EnrollmentPolicy {
    method: &'static str,
    principal: PrincipalIdentity,
    role: Role,
    grants: Vec<Value>,
}
/// Rotation may expose several API key hashes for one logical credential, only
/// when their complete policy agrees. Other shared IDs are ambiguous and refused.
pub(super) fn validate_enrollments(config: &AuthFileConfig) -> Result<(), String> {
    let mut ids = BTreeMap::new();
    for (id, principal, role, grants, method) in config
        .users
        .iter()
        .map(|u| {
            (
                u.authority_id.as_deref(),
                u.principal.as_ref(),
                &u.role,
                &u.grants,
                "jwt",
            )
        })
        .chain(config.api_keys.iter().map(|k| {
            (
                k.authority_id.as_deref(),
                k.principal.as_ref(),
                &k.role,
                &k.grants,
                "api_key",
            )
        }))
    {
        let Some(id) = id else {
            continue;
        };
        if !valid_id(id) {
            return Err("invalid credential authority ID".into());
        }
        let policy = EnrollmentPolicy {
            method,
            principal: principal
                .ok_or("credential authority ID requires a stable principal")?
                .clone(),
            role: Role::from_str_loose(role).ok_or("invalid enrollment role")?,
            grants: canonical_grants(grants)?,
        };
        if let Some(previous) = ids.get(id)
            && (method != "api_key" || previous != &policy)
        {
            return Err("credential authority ID has conflicting enrollments".into());
        }
        ids.insert(id, policy);
    }
    Ok(())
}
