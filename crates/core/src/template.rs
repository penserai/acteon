use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum length of a template or profile name.
const MAX_NAME_LEN: usize = 128;

/// Maximum size of template content in bytes (512 KB).
const MAX_CONTENT_BYTES: usize = 512 * 1024;

/// A reusable `MiniJinja` template stored in the system.
///
/// Templates contain raw Jinja2-compatible text that is rendered at dispatch
/// time with action payload variables. They are scoped to a namespace + tenant
/// pair and referenced by name from [`TemplateProfile`] fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Template {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Template name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this template belongs to.
    pub namespace: String,
    /// Tenant this template belongs to.
    pub tenant: String,
    /// Raw `MiniJinja` template content.
    pub content: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// When this template was created.
    pub created_at: DateTime<Utc>,
    /// When this template was last updated.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary key-value labels for filtering and organization.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// A template profile maps payload fields to template content.
///
/// At dispatch time, if an action's `template` field matches a profile name,
/// each field mapping is rendered using the action payload as variables and
/// the results are merged into the payload before provider execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TemplateProfile {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Profile name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this profile belongs to.
    pub namespace: String,
    /// Tenant this profile belongs to.
    pub tenant: String,
    /// Field-to-template mappings. Keys are target payload field names.
    pub fields: HashMap<String, TemplateProfileField>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// When this profile was created.
    pub created_at: DateTime<Utc>,
    /// When this profile was last updated.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary key-value labels for filtering and organization.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// A field mapping within a [`TemplateProfile`].
///
/// Each field is either an inline Jinja literal or a reference (`$ref`) to a
/// stored [`Template`] by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum TemplateProfileField {
    /// Inline Jinja template literal.
    Inline(String),
    /// Reference to a stored template by name.
    Ref {
        /// Name of the stored template to render.
        #[serde(rename = "$ref")]
        template_ref: String,
    },
}

/// Validate a template or profile name.
///
/// Names must be 1-128 characters, using only alphanumeric characters, hyphens,
/// underscores, and dots.
pub fn validate_template_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("template name must not be empty".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!(
            "template name exceeds maximum length of {MAX_NAME_LEN} characters"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(
            "template name must contain only alphanumeric characters, hyphens, underscores, and dots".to_string(),
        );
    }
    Ok(())
}

/// Validate template content size.
///
/// Content must not exceed 512 KB.
pub fn validate_template_content(content: &str) -> Result<(), String> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(format!(
            "template content exceeds maximum size of {MAX_CONTENT_BYTES} bytes"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_serde_roundtrip() {
        let template = Template {
            id: "tpl-001".into(),
            name: "welcome-email".into(),
            namespace: "notifications".into(),
            tenant: "tenant-1".into(),
            content: "Hello {{ name }}, welcome to {{ company }}!".into(),
            description: Some("Welcome email body".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&template).unwrap();
        let back: Template = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "tpl-001");
        assert_eq!(back.name, "welcome-email");
        assert!(back.content.contains("{{ name }}"));
    }

    #[test]
    fn template_profile_serde_roundtrip() {
        let mut fields = HashMap::new();
        fields.insert(
            "subject".to_string(),
            TemplateProfileField::Inline("Welcome, {{ user_name }}!".to_string()),
        );
        fields.insert(
            "body".to_string(),
            TemplateProfileField::Ref {
                template_ref: "welcome-email".to_string(),
            },
        );

        let profile = TemplateProfile {
            id: "prof-001".into(),
            name: "welcome-profile".into(),
            namespace: "notifications".into(),
            tenant: "tenant-1".into(),
            fields,
            description: Some("Welcome email profile".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&profile).unwrap();
        let back: TemplateProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "prof-001");
        assert_eq!(back.fields.len(), 2);
    }

    #[test]
    fn untagged_field_inline() {
        let json = r#""Hello {{ name }}""#;
        let field: TemplateProfileField = serde_json::from_str(json).unwrap();
        assert!(matches!(field, TemplateProfileField::Inline(s) if s.contains("{{ name }}")));
    }

    #[test]
    fn untagged_field_ref() {
        let json = r#"{"$ref": "welcome-email"}"#;
        let field: TemplateProfileField = serde_json::from_str(json).unwrap();
        assert!(
            matches!(field, TemplateProfileField::Ref { template_ref } if template_ref == "welcome-email")
        );
    }

    #[test]
    fn validate_name_valid() {
        assert!(validate_template_name("welcome-email").is_ok());
        assert!(validate_template_name("my_template.v1").is_ok());
        assert!(validate_template_name("a").is_ok());
    }

    #[test]
    fn validate_name_empty() {
        assert!(validate_template_name("").is_err());
    }

    #[test]
    fn validate_name_too_long() {
        let long_name = "a".repeat(MAX_NAME_LEN + 1);
        assert!(validate_template_name(&long_name).is_err());
    }

    #[test]
    fn validate_name_invalid_chars() {
        assert!(validate_template_name("hello world").is_err());
        assert!(validate_template_name("hello/world").is_err());
    }

    #[test]
    fn validate_content_valid() {
        assert!(validate_template_content("Hello {{ name }}").is_ok());
    }

    #[test]
    fn validate_content_too_large() {
        let large = "x".repeat(MAX_CONTENT_BYTES + 1);
        assert!(validate_template_content(&large).is_err());
    }

    #[test]
    fn template_deserializes_with_defaults() {
        let json = r#"{
            "id": "tpl-002",
            "name": "test",
            "namespace": "ns",
            "tenant": "t",
            "content": "Hello",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let template: Template = serde_json::from_str(json).unwrap();
        assert!(template.description.is_none());
        assert!(template.labels.is_empty());
    }
}
