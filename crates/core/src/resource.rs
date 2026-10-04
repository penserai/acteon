//! Exact, tenant-scoped resource identity for execution authority.
//!
//! A reference identifies a resource; it does not grant access to it. Resource
//! selectors and authoritative endpoint resolution are separate concerns.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Maximum UTF-8 bytes in a namespace or tenant component.
pub const MAX_RESOURCE_SCOPE_BYTES: usize = 255;
/// Maximum UTF-8 bytes in the opaque resource identifier.
pub const MAX_RESOURCE_ID_BYTES: usize = 1024;
const PREFIX: &str = "acteon-resource:v1:";

/// Closed set of resource categories understood by this format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Agent,
    Provider,
    Action,
    Topic,
    Subscription,
    Chain,
    Workflow,
    ExternalService,
    Model,
    Skill,
    Endpoint,
    Route,
}

impl ResourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Provider => "provider",
            Self::Action => "action",
            Self::Topic => "topic",
            Self::Subscription => "subscription",
            Self::Chain => "chain",
            Self::Workflow => "workflow",
            Self::ExternalService => "external_service",
            Self::Model => "model",
            Self::Skill => "skill",
            Self::Endpoint => "endpoint",
            Self::Route => "route",
        }
    }
}

impl FromStr for ResourceKind {
    type Err = ResourceRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "agent" => Ok(Self::Agent),
            "provider" => Ok(Self::Provider),
            "action" => Ok(Self::Action),
            "topic" => Ok(Self::Topic),
            "subscription" => Ok(Self::Subscription),
            "chain" => Ok(Self::Chain),
            "workflow" => Ok(Self::Workflow),
            "external_service" => Ok(Self::ExternalService),
            "model" => Ok(Self::Model),
            "skill" => Ok(Self::Skill),
            "endpoint" => Ok(Self::Endpoint),
            "route" => Ok(Self::Route),
            _ => Err(ResourceRefError::Encoding),
        }
    }
}

/// Validated exact identity. Private fields keep construction and deserialization
/// subject to the same bounds. Tenant hierarchy and wildcard matching do not
/// apply to equality or the canonical encoding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(try_from = "ResourceRefWire")]
pub struct ResourceRef {
    kind: ResourceKind,
    namespace: String,
    tenant: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceRefWire {
    kind: ResourceKind,
    namespace: String,
    tenant: String,
    id: String,
}

impl TryFrom<ResourceRefWire> for ResourceRef {
    type Error = ResourceRefError;

    fn try_from(value: ResourceRefWire) -> Result<Self, Self::Error> {
        Self::new(value.kind, value.namespace, value.tenant, value.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceRefError {
    #[error("invalid resource component: {0}")]
    Component(&'static str),
    #[error("invalid or unsupported canonical resource encoding")]
    Encoding,
}

fn valid_component(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value != "*"
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

impl ResourceRef {
    pub fn new(
        kind: ResourceKind,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, ResourceRefError> {
        let namespace = namespace.into();
        let tenant = tenant.into();
        let id = id.into();
        for (name, value, max) in [
            ("namespace", namespace.as_str(), MAX_RESOURCE_SCOPE_BYTES),
            ("tenant", tenant.as_str(), MAX_RESOURCE_SCOPE_BYTES),
            ("id", id.as_str(), MAX_RESOURCE_ID_BYTES),
        ] {
            if !valid_component(value, max) {
                return Err(ResourceRefError::Component(name));
            }
        }
        Ok(Self {
            kind,
            namespace,
            tenant,
            id,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Versioned storage/audit identity. Lowercase hex encodes UTF-8 components
    /// independently, so delimiters and Unicode cannot shift scope boundaries.
    #[must_use]
    pub fn canonical(&self) -> String {
        format!(
            "{PREFIX}{}:{}:{}:{}",
            self.kind.as_str(),
            hex::encode(&self.namespace),
            hex::encode(&self.tenant),
            hex::encode(&self.id)
        )
    }
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

impl FromStr for ResourceRef {
    type Err = ResourceRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Bound before decoding allocations. The wire has three hex components.
        let max = PREFIX.len() + 32 + 2 * (2 * MAX_RESOURCE_SCOPE_BYTES + MAX_RESOURCE_ID_BYTES);
        if value.len() > max {
            return Err(ResourceRefError::Encoding);
        }
        let body = value
            .strip_prefix(PREFIX)
            .ok_or(ResourceRefError::Encoding)?;
        let fields: Vec<_> = body.split(':').collect();
        if fields.len() != 4 {
            return Err(ResourceRefError::Encoding);
        }
        let decode = |field: &str| {
            String::from_utf8(hex::decode(field).map_err(|_| ResourceRefError::Encoding)?)
                .map_err(|_| ResourceRefError::Encoding)
        };
        let result = Self::new(
            fields[0].parse()?,
            decode(fields[1])?,
            decode(fields[2])?,
            decode(fields[3])?,
        )?;
        // One spelling per identity: reject alternate hex case and future aliases.
        if result.canonical() != value {
            return Err(ResourceRefError::Encoding);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiters_unicode_and_tenant_hierarchy_remain_exact() {
        let cases = [
            ("a:b", "c", "d"),
            ("a", "b:c", "d"),
            ("a", "b", "c:d"),
            ("a", "acme", "d"),
            ("a", "acme.prod", "d"),
            ("a", "acme", "δοκιμή/雪"),
        ];
        let refs: Vec<_> = cases
            .into_iter()
            .map(|(ns, tenant, id)| {
                ResourceRef::new(ResourceKind::Provider, ns, tenant, id).unwrap()
            })
            .collect();
        for (i, resource) in refs.iter().enumerate() {
            assert_eq!(
                resource.canonical().parse::<ResourceRef>().unwrap(),
                *resource
            );
            assert_eq!(
                serde_json::from_str::<ResourceRef>(&serde_json::to_string(resource).unwrap())
                    .unwrap(),
                *resource
            );
            for other in refs.iter().skip(i + 1) {
                assert_ne!(resource.canonical(), other.canonical());
            }
        }
        let agent = ResourceRef::new(ResourceKind::Agent, "a", "acme", "d").unwrap();
        assert_ne!(agent.canonical(), refs[3].canonical());
    }

    #[test]
    fn malformed_and_future_encodings_fail_closed() {
        let valid = ResourceRef::new(ResourceKind::Agent, "scope", "tenant", "peer")
            .unwrap()
            .canonical();
        for input in [
            valid.replace(":v1:", ":v2:"),
            valid.replace(":agent:", ":future:"),
            valid.to_uppercase(),
            valid.replace('f', "F"),
            format!("{valid}:extra"),
            "acteon-resource:v1:agent:ff:74:69".into(),
            "acteon-resource:v1:agent:zz:74:69".into(),
            "acteon-resource:v1:agent::74:69".into(),
        ] {
            assert!(input.parse::<ResourceRef>().is_err(), "accepted {input}");
        }
    }

    #[test]
    fn payloads_cannot_bypass_constructor_validation() {
        for value in ["", "*", " leading", "trailing ", "bad\ncomponent"] {
            assert!(ResourceRef::new(ResourceKind::Topic, value, "tenant", "id").is_err());
            let wire = serde_json::json!({"kind":"topic","namespace":"scope","tenant":"tenant","id":value});
            assert!(serde_json::from_value::<ResourceRef>(wire).is_err());
        }
        let mut wire =
            serde_json::json!({"kind":"topic","namespace":"scope","tenant":"tenant","id":"id"});
        wire["authority"] = true.into();
        assert!(serde_json::from_value::<ResourceRef>(wire).is_err());
        assert!(
            ResourceRef::new(ResourceKind::Topic, "scope", "tenant", "雪".repeat(342)).is_err()
        );
    }
}
