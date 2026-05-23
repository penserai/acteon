//! A2A `AgentCard` and supporting types (Phase 1).
//!
//! The `AgentCard` is the *advertisement* an agent publishes for
//! external discovery (`GET /.well-known/agent.json` in Phase 3). It
//! declares capabilities, skills (with input schemas), wire
//! interfaces, and accepted security schemes.
//!
//! **Why this lives in a separate module from [`crate::bus_agent`]:**
//! A2A's card schema is verbose (skills with full JSON Schema input
//! definitions, multiple security schemes, extension URIs). The
//! [`crate::bus_agent::Agent`] struct is on the hot path — every
//! heartbeat, every routing decision, every listing reads it.
//! Inlining the card fields onto `Agent` would bloat that hot path
//! for tenants with many agents. So [`crate::bus_agent::Agent`] keeps a thin
//! `has_agent_card: bool` flag, and the full card lives at a
//! separate state-store key (Phase 2 wires `KeyKind::BusAgentCard`)
//! fetched only when an A2A discovery request hits.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// Caps
// ---------------------------------------------------------------------

/// Max length of a card's `description` field.
pub const MAX_CARD_DESCRIPTION_BYTES: usize = 4 * 1024;

/// Max length of a `Skill::description`.
pub const MAX_SKILL_DESCRIPTION_BYTES: usize = 2 * 1024;

/// Max serialized size of a `Skill::input_schema` JSON Schema.
pub const MAX_SKILL_INPUT_SCHEMA_BYTES: usize = 64 * 1024;

/// Max number of skills per card.
pub const MAX_SKILLS_PER_CARD: usize = 64;

/// Max number of wire interfaces per card.
pub const MAX_INTERFACES_PER_CARD: usize = 16;

/// Max number of declared extensions per card.
pub const MAX_EXTENSIONS_PER_CARD: usize = 32;

/// Max number of security schemes per card.
pub const MAX_SECURITY_SCHEMES_PER_CARD: usize = 16;

/// Max number of output media types per skill.
pub const MAX_OUTPUT_MEDIA_TYPES_PER_SKILL: usize = 16;

// ---------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------

/// The organization / publisher behind an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Provider {
    /// Human-readable provider name (e.g. `"Acme Corp"`).
    pub name: String,
    /// Provider home page or identity URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------
// AgentCapabilities
// ---------------------------------------------------------------------

/// Capability flags an agent advertises. These drive A2A client
/// behavior — e.g. an A2A client won't attempt `SendStreamingMessage`
/// against an agent whose card has `streaming = false`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentCapabilities {
    /// Agent supports SSE streaming (`SendStreamingMessage`,
    /// `SubscribeToTask`).
    #[serde(default)]
    pub streaming: bool,
    /// Agent supports per-task push notification configs.
    #[serde(default)]
    pub push_notifications: bool,
    /// Agent exposes an extended card via `GetExtendedAgentCard`
    /// (richer than the public well-known card).
    #[serde(default)]
    pub extended_agent_card: bool,
}

// ---------------------------------------------------------------------
// Skill
// ---------------------------------------------------------------------

/// A discrete capability an agent exposes, with optional JSON Schema
/// describing valid inputs and a list of media types it can return.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Skill {
    /// Skill name. Used as the canonical token for invocation.
    pub name: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing valid input shape. Stored as a JSON
    /// value rather than a typed schema so we don't pull a schema
    /// crate into core just for this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<Object>))]
    pub input_schema: Option<serde_json::Value>,
    /// IANA media types this skill can return (e.g. `text/plain`,
    /// `application/json`, `image/png`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_media_types: Vec<String>,
}

impl Skill {
    /// Construct a skill with just a name. Build up via field access.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            input_schema: None,
            output_media_types: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), AgentCardValidationError> {
        validate_token("skill.name", &self.name)?;
        if let Some(d) = &self.description
            && d.len() > MAX_SKILL_DESCRIPTION_BYTES
        {
            return Err(AgentCardValidationError::SkillDescriptionTooLong);
        }
        if let Some(schema) = &self.input_schema {
            let encoded = serde_json::to_vec(schema)
                .map_err(|_| AgentCardValidationError::SkillInputSchemaInvalid)?;
            if encoded.len() > MAX_SKILL_INPUT_SCHEMA_BYTES {
                return Err(AgentCardValidationError::SkillInputSchemaTooLong);
            }
        }
        if self.output_media_types.len() > MAX_OUTPUT_MEDIA_TYPES_PER_SKILL {
            return Err(AgentCardValidationError::TooManyOutputMediaTypes);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Interface
// ---------------------------------------------------------------------

/// Transport binding the agent supports. `kind` is one of `"json-rpc"`,
/// `"grpc"`, `"rest"` — A2A spec §8.3.1.
///
/// (A2A's wire field is `"type"`, but `type` is a Rust keyword;
/// we serialize the same wire shape via `#[serde(rename = "type")]`.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Interface {
    /// Transport kind (`"json-rpc"`, `"grpc"`, `"rest"`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Endpoint URL the agent serves this interface at.
    pub url: String,
}

impl Interface {
    fn validate(&self) -> Result<(), AgentCardValidationError> {
        if self.kind.is_empty() {
            return Err(AgentCardValidationError::EmptyInterfaceKind);
        }
        if self.url.is_empty() {
            return Err(AgentCardValidationError::EmptyInterfaceUrl);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// SecurityScheme
// ---------------------------------------------------------------------

/// One of the five A2A-supported security schemes (spec §4.5).
/// Tagged on `scheme` so the wire shape is
/// `{"scheme": "api_key", "name": "X-API-Key", "in": "header"}`.
///
/// Phase 4 wires these to Acteon's existing API-key grants, Bearer
/// tokens, and mTLS stack. `OAuth2` / `OpenIdConnect` are scoped out
/// of the MVP but defined here so the type model is complete and
/// future-additive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SecurityScheme {
    /// API key in header, query, or cookie.
    ApiKey {
        /// Parameter name (e.g. `"X-API-Key"`).
        name: String,
        /// Where to find the key (`"header"`, `"query"`, `"cookie"`).
        #[serde(rename = "in")]
        location: String,
    },
    /// HTTP `Authorization` header (Basic, Bearer, etc.).
    HttpAuth {
        /// HTTP authentication scheme (`"basic"`, `"bearer"`, etc.).
        scheme_name: String,
        /// For Bearer: hint at token format (e.g. `"JWT"`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_format: Option<String>,
    },
    /// OAuth 2.0 with declared flows. Flows stored as opaque JSON
    /// because the A2A spec defers to the OAuth 2.0 flow object
    /// shape and we don't want to drag in an OAuth crate here.
    OAuth2 {
        #[cfg_attr(feature = "openapi", schema(value_type = Object))]
        flows: serde_json::Value,
    },
    /// `OpenID` Connect discovery URL.
    OpenIdConnect { open_id_connect_url: String },
    /// Mutual TLS (client certificate validated server-side).
    MutualTls,
}

impl SecurityScheme {
    fn validate(&self) -> Result<(), AgentCardValidationError> {
        match self {
            SecurityScheme::ApiKey { name, location } => {
                if name.is_empty() {
                    return Err(AgentCardValidationError::EmptySecurityField("apiKey.name"));
                }
                if !matches!(location.as_str(), "header" | "query" | "cookie") {
                    return Err(AgentCardValidationError::InvalidSecurityLocation(
                        location.clone(),
                    ));
                }
            }
            SecurityScheme::HttpAuth { scheme_name, .. } => {
                if scheme_name.is_empty() {
                    return Err(AgentCardValidationError::EmptySecurityField(
                        "httpAuth.schemeName",
                    ));
                }
            }
            SecurityScheme::OpenIdConnect {
                open_id_connect_url,
            } => {
                if open_id_connect_url.is_empty() {
                    return Err(AgentCardValidationError::EmptySecurityField(
                        "openIdConnect.openIdConnectUrl",
                    ));
                }
            }
            SecurityScheme::OAuth2 { .. } | SecurityScheme::MutualTls => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Extension
// ---------------------------------------------------------------------

/// A2A protocol extension the agent participates in (spec §4.4.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Extension {
    /// Extension identifier URI.
    pub uri: String,
    /// Version string.
    pub version: String,
    /// If true, clients MUST declare support in `A2A-Extensions`
    /// header or be rejected.
    #[serde(default)]
    pub required: bool,
}

impl Extension {
    fn validate(&self) -> Result<(), AgentCardValidationError> {
        if self.uri.is_empty() {
            return Err(AgentCardValidationError::EmptySecurityField(
                "extension.uri",
            ));
        }
        if self.version.is_empty() {
            return Err(AgentCardValidationError::EmptySecurityField(
                "extension.version",
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// AgentCard
// ---------------------------------------------------------------------

/// A2A `AgentCard` — the discovery advertisement for an agent. Lives at
/// a separate state-store key from the hot
/// [`crate::bus_agent::Agent`] record (Phase 2 wires
/// `KeyKind::BusAgentCard`).
///
/// Identity fields (`agent_id`, `namespace`, `tenant`) match the
/// corresponding `Agent` so the card and the hot-path record join
/// cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentCard {
    /// Agent identifier. Same as `Agent.agent_id`.
    pub agent_id: String,
    /// Namespace. Same as `Agent.namespace`.
    pub namespace: String,
    /// Tenant. Same as `Agent.tenant`.
    pub tenant: String,
    /// Human-readable agent name (often distinct from `agent_id`,
    /// which may be a slug).
    pub name: String,
    /// Free-text agent description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Publishing provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    /// Capability flags.
    #[serde(default)]
    pub capabilities: AgentCapabilities,
    /// Skills the agent exposes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<Skill>,
    /// Transport interfaces the agent supports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<Interface>,
    /// Named security schemes this agent accepts. Keyed by an
    /// operator-chosen alias so a client can refer to a specific
    /// scheme by name in its request.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub security_schemes: HashMap<String, SecurityScheme>,
    /// Protocol extensions the agent participates in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<Extension>,
    /// Card version (operator-controlled; changes when capabilities,
    /// skills, or schemes change in a backwards-incompatible way).
    pub version: String,
    /// Optional cryptographic signature over the card body. Card
    /// signing is deferred to Phase 4 hardening; the field is here so
    /// the wire shape is stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Card creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last mutation timestamp.
    pub updated_at: DateTime<Utc>,
}

impl AgentCard {
    /// Construct a minimal card with the required identity fields and
    /// an operator-supplied name + version. Capabilities default to
    /// none-enabled; populate before publishing.
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            agent_id: agent_id.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            name: name.into(),
            description: None,
            provider: None,
            capabilities: AgentCapabilities::default(),
            skills: Vec::new(),
            interfaces: Vec::new(),
            security_schemes: HashMap::new(),
            extensions: Vec::new(),
            version: version.into(),
            signature: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Validate identity, bounded fields, and every nested record.
    pub fn validate(&self) -> Result<(), AgentCardValidationError> {
        validate_id("agentId", &self.agent_id)?;
        validate_fragment("namespace", &self.namespace)?;
        validate_fragment("tenant", &self.tenant)?;
        if self.name.is_empty() {
            return Err(AgentCardValidationError::EmptyName);
        }
        if self.name.len() > 256 {
            return Err(AgentCardValidationError::NameTooLong);
        }
        if let Some(d) = &self.description
            && d.len() > MAX_CARD_DESCRIPTION_BYTES
        {
            return Err(AgentCardValidationError::DescriptionTooLong);
        }
        if self.version.is_empty() {
            return Err(AgentCardValidationError::EmptyVersion);
        }
        if self.skills.len() > MAX_SKILLS_PER_CARD {
            return Err(AgentCardValidationError::TooManySkills);
        }
        for s in &self.skills {
            s.validate()?;
        }
        if self.interfaces.len() > MAX_INTERFACES_PER_CARD {
            return Err(AgentCardValidationError::TooManyInterfaces);
        }
        for i in &self.interfaces {
            i.validate()?;
        }
        if self.extensions.len() > MAX_EXTENSIONS_PER_CARD {
            return Err(AgentCardValidationError::TooManyExtensions);
        }
        for e in &self.extensions {
            e.validate()?;
        }
        if self.security_schemes.len() > MAX_SECURITY_SCHEMES_PER_CARD {
            return Err(AgentCardValidationError::TooManySecuritySchemes);
        }
        for (k, scheme) in &self.security_schemes {
            if k.is_empty() {
                return Err(AgentCardValidationError::EmptySecuritySchemeKey);
            }
            scheme.validate()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------

fn validate_id(field: &'static str, s: &str) -> Result<(), AgentCardValidationError> {
    if s.is_empty() {
        return Err(AgentCardValidationError::EmptyId(field));
    }
    if s.len() > 120 {
        return Err(AgentCardValidationError::IdTooLong(field));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(AgentCardValidationError::InvalidIdChar {
            field,
            value: s.to_string(),
        });
    }
    Ok(())
}

fn validate_fragment(field: &'static str, s: &str) -> Result<(), AgentCardValidationError> {
    if s.is_empty() {
        return Err(AgentCardValidationError::EmptyFragment(field));
    }
    if s.len() > 80 {
        return Err(AgentCardValidationError::FragmentTooLong(field));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AgentCardValidationError::InvalidFragmentChar {
            field,
            value: s.to_string(),
        });
    }
    Ok(())
}

fn validate_token(field: &'static str, s: &str) -> Result<(), AgentCardValidationError> {
    if s.is_empty() {
        return Err(AgentCardValidationError::EmptyId(field));
    }
    if s.len() > 120 {
        return Err(AgentCardValidationError::IdTooLong(field));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(AgentCardValidationError::InvalidIdChar {
            field,
            value: s.to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgentCardValidationError {
    #[error("{0} must not be empty")]
    EmptyId(&'static str),
    #[error("{0} exceeds 120 characters")]
    IdTooLong(&'static str),
    #[error("{field} '{value}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar { field: &'static str, value: String },
    #[error("{0} must not be empty")]
    EmptyFragment(&'static str),
    #[error("{0} exceeds 80 characters")]
    FragmentTooLong(&'static str),
    #[error("{field} '{value}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidFragmentChar { field: &'static str, value: String },
    #[error("agent card name must not be empty")]
    EmptyName,
    #[error("agent card name exceeds 256 characters")]
    NameTooLong,
    #[error("agent card description exceeds {MAX_CARD_DESCRIPTION_BYTES} bytes")]
    DescriptionTooLong,
    #[error("agent card version must not be empty")]
    EmptyVersion,
    #[error("skill description exceeds {MAX_SKILL_DESCRIPTION_BYTES} bytes")]
    SkillDescriptionTooLong,
    #[error("skill inputSchema is not serializable JSON")]
    SkillInputSchemaInvalid,
    #[error("skill inputSchema exceeds {MAX_SKILL_INPUT_SCHEMA_BYTES} bytes")]
    SkillInputSchemaTooLong,
    #[error("skill outputMediaTypes exceed {MAX_OUTPUT_MEDIA_TYPES_PER_SKILL}")]
    TooManyOutputMediaTypes,
    #[error("skills exceed {MAX_SKILLS_PER_CARD}")]
    TooManySkills,
    #[error("interfaces exceed {MAX_INTERFACES_PER_CARD}")]
    TooManyInterfaces,
    #[error("interface kind must not be empty")]
    EmptyInterfaceKind,
    #[error("interface url must not be empty")]
    EmptyInterfaceUrl,
    #[error("extensions exceed {MAX_EXTENSIONS_PER_CARD}")]
    TooManyExtensions,
    #[error("securitySchemes exceed {MAX_SECURITY_SCHEMES_PER_CARD}")]
    TooManySecuritySchemes,
    #[error("security scheme key must not be empty")]
    EmptySecuritySchemeKey,
    #[error("required security field {0} must not be empty")]
    EmptySecurityField(&'static str),
    #[error("apiKey 'in' field must be header/query/cookie (got '{0}')")]
    InvalidSecurityLocation(String),
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> AgentCard {
        AgentCard::new("planner-1", "agents", "demo", "Planner", "1.0.0")
    }

    #[test]
    fn minimal_card_validates() {
        sample().validate().unwrap();
    }

    #[test]
    fn rejects_empty_name() {
        let mut c = sample();
        c.name = String::new();
        assert_eq!(c.validate(), Err(AgentCardValidationError::EmptyName));
    }

    #[test]
    fn rejects_empty_version() {
        let mut c = sample();
        c.version = String::new();
        assert_eq!(c.validate(), Err(AgentCardValidationError::EmptyVersion));
    }

    #[test]
    fn rejects_bad_agent_id() {
        let mut c = sample();
        c.agent_id = "bad/id".into();
        assert!(matches!(
            c.validate(),
            Err(AgentCardValidationError::InvalidIdChar {
                field: "agentId",
                ..
            })
        ));
    }

    #[test]
    fn rejects_long_description() {
        let mut c = sample();
        c.description = Some("x".repeat(MAX_CARD_DESCRIPTION_BYTES + 1));
        assert_eq!(
            c.validate(),
            Err(AgentCardValidationError::DescriptionTooLong)
        );
    }

    #[test]
    fn validates_skill_input_schema() {
        let mut c = sample();
        let mut skill = Skill::new("summarize");
        skill.input_schema = Some(json!({"type": "object"}));
        skill.output_media_types = vec!["text/plain".into()];
        c.skills.push(skill);
        c.validate().unwrap();
    }

    #[test]
    fn rejects_oversized_skill_input_schema() {
        let mut c = sample();
        let mut skill = Skill::new("summarize");
        // Build a JSON object whose serialized form exceeds the cap.
        let big = "x".repeat(MAX_SKILL_INPUT_SCHEMA_BYTES);
        skill.input_schema = Some(json!({"big": big}));
        c.skills.push(skill);
        assert_eq!(
            c.validate(),
            Err(AgentCardValidationError::SkillInputSchemaTooLong)
        );
    }

    #[test]
    fn caps_skills_count() {
        let mut c = sample();
        for i in 0..=MAX_SKILLS_PER_CARD {
            c.skills.push(Skill::new(format!("skill-{i}")));
        }
        assert_eq!(c.validate(), Err(AgentCardValidationError::TooManySkills));
    }

    #[test]
    fn validates_security_scheme_api_key() {
        let mut c = sample();
        c.security_schemes.insert(
            "primary".into(),
            SecurityScheme::ApiKey {
                name: "X-API-Key".into(),
                location: "header".into(),
            },
        );
        c.validate().unwrap();
    }

    #[test]
    fn rejects_bad_api_key_location() {
        let mut c = sample();
        c.security_schemes.insert(
            "primary".into(),
            SecurityScheme::ApiKey {
                name: "X-API-Key".into(),
                location: "body".into(),
            },
        );
        assert!(matches!(
            c.validate(),
            Err(AgentCardValidationError::InvalidSecurityLocation(_))
        ));
    }

    #[test]
    fn rejects_empty_security_scheme_key() {
        let mut c = sample();
        c.security_schemes
            .insert(String::new(), SecurityScheme::MutualTls);
        assert_eq!(
            c.validate(),
            Err(AgentCardValidationError::EmptySecuritySchemeKey)
        );
    }

    #[test]
    fn validates_interface() {
        let mut c = sample();
        c.interfaces.push(Interface {
            kind: "json-rpc".into(),
            url: "https://example.com/a2a/rpc".into(),
        });
        c.validate().unwrap();
    }

    #[test]
    fn rejects_empty_interface_kind() {
        let mut c = sample();
        c.interfaces.push(Interface {
            kind: String::new(),
            url: "https://x".into(),
        });
        assert_eq!(
            c.validate(),
            Err(AgentCardValidationError::EmptyInterfaceKind)
        );
    }

    #[test]
    fn extension_validates() {
        let mut c = sample();
        c.extensions.push(Extension {
            uri: "https://example.com/ext/v1".into(),
            version: "1.0".into(),
            required: true,
        });
        c.validate().unwrap();
    }

    #[test]
    fn capability_flags_serialize_camel_case() {
        let mut c = sample();
        c.capabilities.streaming = true;
        c.capabilities.push_notifications = true;
        c.capabilities.extended_agent_card = false;
        let v = serde_json::to_value(&c.capabilities).unwrap();
        assert_eq!(v.get("streaming"), Some(&json!(true)));
        assert_eq!(v.get("pushNotifications"), Some(&json!(true)));
        assert_eq!(v.get("extendedAgentCard"), Some(&json!(false)));
    }

    #[test]
    fn card_serializes_camel_case() {
        let mut c = sample();
        c.description = Some("a test agent".into());
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("agentId").is_some());
        assert!(v.get("createdAt").is_some());
        assert!(v.get("updatedAt").is_some());
        assert!(v.get("securitySchemes").is_none() || v["securitySchemes"].is_object());
    }

    #[test]
    fn security_scheme_api_key_serializes_with_in_field() {
        let scheme = SecurityScheme::ApiKey {
            name: "X-API-Key".into(),
            location: "header".into(),
        };
        let v = serde_json::to_value(&scheme).unwrap();
        // Wire format must use `in` (A2A spec), not `location`.
        assert_eq!(v.get("in"), Some(&json!("header")));
        assert_eq!(v.get("scheme"), Some(&json!("api_key")));
    }

    #[test]
    fn interface_serializes_with_type_field() {
        let iface = Interface {
            kind: "json-rpc".into(),
            url: "https://x".into(),
        };
        let v = serde_json::to_value(&iface).unwrap();
        // Wire field is `type`, not `kind`.
        assert_eq!(v.get("type"), Some(&json!("json-rpc")));
        assert!(v.get("kind").is_none());
    }

    #[test]
    fn card_roundtrip_serde() {
        let mut c = sample();
        c.skills.push(Skill::new("summarize"));
        c.interfaces.push(Interface {
            kind: "json-rpc".into(),
            url: "https://x".into(),
        });
        c.security_schemes
            .insert("primary".into(), SecurityScheme::MutualTls);
        let j = serde_json::to_string(&c).unwrap();
        let back: AgentCard = serde_json::from_str(&j).unwrap();
        assert_eq!(back.skills.len(), 1);
        assert_eq!(back.interfaces.len(), 1);
        assert_eq!(back.security_schemes.len(), 1);
    }
}
