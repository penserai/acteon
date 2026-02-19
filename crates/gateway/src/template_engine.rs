//! Template rendering engine for payload templating.
//!
//! Renders [`TemplateProfile`] field mappings against action payloads using
//! `MiniJinja` (Jinja2-compatible). Supports both inline template literals and
//! `$ref` references to stored [`Template`] objects.

use std::collections::HashMap;

use acteon_core::template::{Template, TemplateProfile, TemplateProfileField};

use crate::error::GatewayError;

/// Maximum rendered output size per field (1 MB).
const MAX_RENDERED_BYTES: usize = 1_024 * 1_024;

/// Fuel limit for `MiniJinja` template evaluation (denial-of-service protection).
const FUEL_LIMIT: u64 = 100_000;

/// Result of rendering a template profile.
#[derive(Debug)]
pub struct RenderResult {
    /// Rendered field values keyed by field name.
    pub fields: HashMap<String, String>,
}

/// Render a template profile against a payload.
///
/// For each field in the profile:
/// - `Inline(literal)` -- renders the literal as a `MiniJinja` template
/// - `Ref { template_ref }` -- looks up the named template in `templates_map`
///   and renders its content
///
/// The `payload` is flattened into template variables so that
/// `{{ field_name }}` accesses top-level payload keys.
pub fn render_profile<S: ::std::hash::BuildHasher>(
    profile: &TemplateProfile,
    templates_map: &HashMap<String, Template, S>,
    payload: &serde_json::Value,
) -> Result<RenderResult, GatewayError> {
    let mut env = minijinja::Environment::new();
    env.set_fuel(Some(FUEL_LIMIT));

    // Register all stored templates referenced by this profile.
    for (field_name, field) in &profile.fields {
        if let TemplateProfileField::Ref { template_ref } = field {
            let template = templates_map.get(template_ref).ok_or_else(|| {
                GatewayError::TemplateRender(format!(
                    "profile '{}' field '{field_name}' references unknown template '{template_ref}'",
                    profile.name
                ))
            })?;
            env.add_template_owned(template_ref.clone(), template.content.clone())
                .map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "syntax error in template '{template_ref}': {e}"
                    ))
                })?;
        }
    }

    // Build the context from the payload.
    let ctx = minijinja::Value::from_serialize(payload);

    let mut rendered_fields = HashMap::new();

    for (field_name, field) in &profile.fields {
        let rendered = match field {
            TemplateProfileField::Inline(literal) => {
                env.render_str(literal, &ctx).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "error rendering inline field '{field_name}' in profile '{}': {e}",
                        profile.name
                    ))
                })?
            }
            TemplateProfileField::Ref { template_ref } => {
                let tmpl = env.get_template(template_ref).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "failed to load template '{template_ref}': {e}"
                    ))
                })?;
                tmpl.render(&ctx).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "error rendering template '{template_ref}' for field '{field_name}' in profile '{}': {e}",
                        profile.name
                    ))
                })?
            }
        };

        if rendered.len() > MAX_RENDERED_BYTES {
            return Err(GatewayError::TemplateRender(format!(
                "rendered output for field '{field_name}' exceeds maximum size of {MAX_RENDERED_BYTES} bytes"
            )));
        }

        rendered_fields.insert(field_name.clone(), rendered);
    }

    Ok(RenderResult {
        fields: rendered_fields,
    })
}

/// Merge rendered template fields into a JSON payload.
///
/// Rendered string values are inserted as JSON string values into the payload
/// object. Existing fields with the same name are overwritten.
///
/// Returns an error if the payload is not a JSON object.
pub fn merge_rendered_into_payload(
    payload: &mut serde_json::Value,
    rendered: &RenderResult,
) -> Result<(), GatewayError> {
    let obj = payload.as_object_mut().ok_or_else(|| {
        GatewayError::TemplateRender(
            "cannot merge template output into a non-object payload".to_string(),
        )
    })?;

    for (field_name, value) in &rendered.fields {
        obj.insert(field_name.clone(), serde_json::Value::String(value.clone()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_template(name: &str, content: &str) -> Template {
        Template {
            id: format!("tpl-{name}"),
            name: name.to_string(),
            namespace: "ns".to_string(),
            tenant: "t".to_string(),
            content: content.to_string(),
            description: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            labels: HashMap::new(),
        }
    }

    fn make_profile(name: &str, fields: HashMap<String, TemplateProfileField>) -> TemplateProfile {
        TemplateProfile {
            id: format!("prof-{name}"),
            name: name.to_string(),
            namespace: "ns".to_string(),
            tenant: "t".to_string(),
            fields,
            description: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            labels: HashMap::new(),
        }
    }

    #[test]
    fn render_inline_field() {
        let mut fields = HashMap::new();
        fields.insert(
            "greeting".to_string(),
            TemplateProfileField::Inline("Hello, {{ name }}!".to_string()),
        );
        let profile = make_profile("test", fields);
        let payload = serde_json::json!({"name": "Alice"});

        let result = render_profile(&profile, &HashMap::new(), &payload).unwrap();
        assert_eq!(result.fields.get("greeting").unwrap(), "Hello, Alice!");
    }

    #[test]
    fn render_ref_field() {
        let template = make_template("welcome", "Welcome to {{ company }}, {{ name }}!");
        let mut templates = HashMap::new();
        templates.insert("welcome".to_string(), template);

        let mut fields = HashMap::new();
        fields.insert(
            "body".to_string(),
            TemplateProfileField::Ref {
                template_ref: "welcome".to_string(),
            },
        );
        let profile = make_profile("test", fields);
        let payload = serde_json::json!({"name": "Bob", "company": "Acme"});

        let result = render_profile(&profile, &templates, &payload).unwrap();
        assert_eq!(result.fields.get("body").unwrap(), "Welcome to Acme, Bob!");
    }

    #[test]
    fn render_mixed_fields() {
        let template = make_template("body-tpl", "<h1>{{ title }}</h1><p>{{ message }}</p>");
        let mut templates = HashMap::new();
        templates.insert("body-tpl".to_string(), template);

        let mut fields = HashMap::new();
        fields.insert(
            "subject".to_string(),
            TemplateProfileField::Inline("Alert: {{ level }}".to_string()),
        );
        fields.insert(
            "body".to_string(),
            TemplateProfileField::Ref {
                template_ref: "body-tpl".to_string(),
            },
        );
        let profile = make_profile("mixed", fields);
        let payload = serde_json::json!({"level": "critical", "title": "Server Down", "message": "Check immediately"});

        let result = render_profile(&profile, &templates, &payload).unwrap();
        assert_eq!(result.fields.get("subject").unwrap(), "Alert: critical");
        assert!(result.fields.get("body").unwrap().contains("Server Down"));
    }

    #[test]
    fn render_with_loops() {
        let mut fields = HashMap::new();
        fields.insert(
            "list".to_string(),
            TemplateProfileField::Inline(
                "{% for item in items %}{{ item }}{% if not loop.last %}, {% endif %}{% endfor %}"
                    .to_string(),
            ),
        );
        let profile = make_profile("loop-test", fields);
        let payload = serde_json::json!({"items": ["a", "b", "c"]});

        let result = render_profile(&profile, &HashMap::new(), &payload).unwrap();
        assert_eq!(result.fields.get("list").unwrap(), "a, b, c");
    }

    #[test]
    fn render_with_conditionals() {
        let mut fields = HashMap::new();
        fields.insert(
            "msg".to_string(),
            TemplateProfileField::Inline(
                "{% if urgent %}URGENT: {% endif %}{{ message }}".to_string(),
            ),
        );
        let profile = make_profile("cond-test", fields);
        let payload = serde_json::json!({"urgent": true, "message": "disk full"});

        let result = render_profile(&profile, &HashMap::new(), &payload).unwrap();
        assert_eq!(result.fields.get("msg").unwrap(), "URGENT: disk full");
    }

    #[test]
    fn render_missing_ref_returns_error() {
        let mut fields = HashMap::new();
        fields.insert(
            "body".to_string(),
            TemplateProfileField::Ref {
                template_ref: "nonexistent".to_string(),
            },
        );
        let profile = make_profile("bad-ref", fields);
        let payload = serde_json::json!({});

        let err = render_profile(&profile, &HashMap::new(), &payload).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn render_syntax_error_returns_error() {
        let mut fields = HashMap::new();
        fields.insert(
            "bad".to_string(),
            TemplateProfileField::Inline("{{ broken".to_string()),
        );
        let profile = make_profile("syntax-err", fields);
        let payload = serde_json::json!({});

        let err = render_profile(&profile, &HashMap::new(), &payload).unwrap_err();
        assert!(err.to_string().contains("error rendering"));
    }

    #[test]
    fn merge_into_payload() {
        let mut payload = serde_json::json!({"existing": "value", "to_overwrite": "old"});
        let mut fields = HashMap::new();
        fields.insert("new_field".to_string(), "rendered content".to_string());
        fields.insert("to_overwrite".to_string(), "new".to_string());
        let result = RenderResult { fields };

        merge_rendered_into_payload(&mut payload, &result).unwrap();

        assert_eq!(payload["existing"], "value");
        assert_eq!(payload["new_field"], "rendered content");
        assert_eq!(payload["to_overwrite"], "new");
    }

    #[test]
    fn merge_into_non_object_fails() {
        let mut payload = serde_json::json!("not an object");
        let result = RenderResult {
            fields: HashMap::new(),
        };

        let err = merge_rendered_into_payload(&mut payload, &result).unwrap_err();
        assert!(err.to_string().contains("non-object"));
    }

    #[test]
    fn render_nested_variables() {
        let mut fields = HashMap::new();
        fields.insert(
            "msg".to_string(),
            TemplateProfileField::Inline("{{ user.name }} ({{ user.email }})".to_string()),
        );
        let profile = make_profile("nested", fields);
        let payload = serde_json::json!({"user": {"name": "Alice", "email": "alice@example.com"}});

        let result = render_profile(&profile, &HashMap::new(), &payload).unwrap();
        assert_eq!(
            result.fields.get("msg").unwrap(),
            "Alice (alice@example.com)"
        );
    }

    #[test]
    fn render_html_content() {
        let template = make_template(
            "email-body",
            "<html><body><h1>{{ title }}</h1><p>{{ content }}</p></body></html>",
        );
        let mut templates = HashMap::new();
        templates.insert("email-body".to_string(), template);

        let mut fields = HashMap::new();
        fields.insert(
            "html_body".to_string(),
            TemplateProfileField::Ref {
                template_ref: "email-body".to_string(),
            },
        );
        let profile = make_profile("html-test", fields);
        let payload = serde_json::json!({"title": "Welcome", "content": "Hello world"});

        let result = render_profile(&profile, &templates, &payload).unwrap();
        let body = result.fields.get("html_body").unwrap();
        assert!(body.contains("<h1>Welcome</h1>"));
        assert!(body.contains("<p>Hello world</p>"));
    }

    #[test]
    fn render_missing_variable_renders_empty() {
        let mut fields = HashMap::new();
        fields.insert(
            "msg".to_string(),
            TemplateProfileField::Inline("Hello {{ name }}".to_string()),
        );
        let profile = make_profile("missing-var", fields);
        let payload = serde_json::json!({});

        // `MiniJinja` renders missing variables as empty strings by default.
        let result = render_profile(&profile, &HashMap::new(), &payload).unwrap();
        assert_eq!(result.fields.get("msg").unwrap(), "Hello ");
    }

    #[test]
    fn render_empty_profile() {
        let profile = make_profile("empty", HashMap::new());
        let payload = serde_json::json!({"foo": "bar"});

        let result = render_profile(&profile, &HashMap::new(), &payload).unwrap();
        assert!(result.fields.is_empty());
    }
}
