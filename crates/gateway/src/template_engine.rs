//! Template rendering engine for payload templating.
//!
//! Renders [`TemplateProfile`] field mappings against action payloads using
//! `MiniJinja` (Jinja2-compatible). Supports both inline template literals and
//! `$ref` references to stored [`Template`] objects.

use std::collections::HashMap;
use std::io::Write;

use acteon_core::template::{Template, TemplateProfile, TemplateProfileField};

use crate::error::GatewayError;

/// Maximum rendered output size per field (1 MB).
const MAX_RENDERED_BYTES: usize = 1_024 * 1_024;

/// Fuel limit for `MiniJinja` template evaluation (denial-of-service protection).
const FUEL_LIMIT: u64 = 100_000;

/// A writer that aborts once a byte-count limit is exceeded.
struct SizeLimitedWriter {
    buf: Vec<u8>,
    limit: usize,
}

impl SizeLimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
        }
    }

    fn into_string(self) -> Result<String, GatewayError> {
        String::from_utf8(self.buf).map_err(|e| {
            GatewayError::TemplateRender(format!("rendered output is not valid UTF-8: {e}"))
        })
    }
}

impl Write for SizeLimitedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len() + data.len() > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "rendered output exceeds size limit",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Result of rendering a template profile.
#[derive(Debug)]
pub struct RenderResult {
    /// Rendered field values keyed by field name.
    pub fields: HashMap<String, String>,
}

/// Build a `MiniJinja` context value from the action payload and attachment
/// metadata.
///
/// Payload fields live at the root level. Two extra keys are injected:
/// - `attachments` -- ordered list of `{id, name, filename, content_type}`
/// - `attachments_by_id` -- map from attachment `id` to the same metadata
///
/// Binary content (`data_base64`) is intentionally excluded to avoid
/// multi-megabyte strings inside the template engine.
fn build_template_context(
    payload: &serde_json::Value,
    attachments: &[acteon_core::Attachment],
) -> minijinja::Value {
    // Start from the payload (must be an object for merge to work).
    let mut root = match payload {
        serde_json::Value::Object(map) => {
            serde_json::Value::Object(map.clone())
        }
        other => {
            // Wrap non-object payloads so we always have a map to inject into.
            let mut map = serde_json::Map::new();
            map.insert("payload".into(), other.clone());
            serde_json::Value::Object(map)
        }
    };

    // Build attachment metadata list (no data_base64).
    let att_list: Vec<serde_json::Value> = attachments
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "name": a.name,
                "filename": a.filename,
                "content_type": a.content_type,
            })
        })
        .collect();

    // Build id → metadata lookup map.
    let att_by_id: serde_json::Map<String, serde_json::Value> = attachments
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                serde_json::json!({
                    "id": a.id,
                    "name": a.name,
                    "filename": a.filename,
                    "content_type": a.content_type,
                }),
            )
        })
        .collect();

    if let serde_json::Value::Object(ref mut map) = root {
        map.insert("attachments".into(), serde_json::Value::Array(att_list));
        map.insert(
            "attachments_by_id".into(),
            serde_json::Value::Object(att_by_id),
        );
    }

    minijinja::Value::from_serialize(&root)
}

/// Render a template profile against a payload and optional attachments.
///
/// For each field in the profile:
/// - `Inline(literal)` -- renders the literal as a `MiniJinja` template
/// - `Ref { template_ref }` -- looks up the named template in `templates_map`
///   and renders its content
///
/// All templates in `templates_map` are registered in the `MiniJinja` environment
/// so that `{% include %}` and `{% extends %}` directives work across templates.
///
/// Attachment metadata (without `data_base64`) is injected into the template
/// context under two keys:
/// - **`attachments`** -- list of `{id, name, filename, content_type}` objects
/// - **`attachments_by_id`** -- map from `id` to the same metadata objects
///
/// Output is streamed through a size-limited writer that aborts mid-render
/// when the per-field limit is exceeded, preventing unbounded memory growth.
pub fn render_profile<S: ::std::hash::BuildHasher>(
    profile: &TemplateProfile,
    templates_map: &HashMap<String, Template, S>,
    payload: &serde_json::Value,
    attachments: &[acteon_core::Attachment],
) -> Result<RenderResult, GatewayError> {
    let mut env = minijinja::Environment::new();
    env.set_fuel(Some(FUEL_LIMIT));

    // Register ALL scoped templates so {% include %} and {% extends %} work.
    for (name, template) in templates_map {
        env.add_template_owned(name.clone(), template.content.clone())
            .map_err(|e| {
                GatewayError::TemplateRender(format!("syntax error in template '{name}': {e}"))
            })?;
    }

    // Validate that all $ref fields reference templates that exist in scope.
    for (field_name, field) in &profile.fields {
        if let TemplateProfileField::Ref { template_ref } = field
            && !templates_map.contains_key(template_ref.as_str())
        {
            return Err(GatewayError::TemplateRender(format!(
                "profile '{}' field '{field_name}' references unknown template '{template_ref}'",
                profile.name
            )));
        }
    }

    // Build the context: payload fields at root level + attachment metadata.
    let ctx = build_template_context(payload, attachments);

    let mut rendered_fields = HashMap::new();

    for (field_name, field) in &profile.fields {
        let rendered = match field {
            TemplateProfileField::Inline(literal) => {
                // Add the inline literal as a named template so we can use
                // render_to_write for streaming size enforcement.
                let inline_name = format!("__inline__{field_name}");
                env.add_template_owned(inline_name.clone(), literal.clone())
                    .map_err(|e| {
                        GatewayError::TemplateRender(format!(
                            "error compiling inline field '{field_name}' in profile '{}': {e}",
                            profile.name
                        ))
                    })?;
                let tmpl = env.get_template(&inline_name).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "error loading inline field '{field_name}': {e}"
                    ))
                })?;
                let mut writer = SizeLimitedWriter::new(MAX_RENDERED_BYTES);
                tmpl.render_to_write(&ctx, &mut writer).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "error rendering inline field '{field_name}' in profile '{}': {e}",
                        profile.name
                    ))
                })?;
                writer.into_string()?
            }
            TemplateProfileField::Ref { template_ref } => {
                let tmpl = env.get_template(template_ref).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "failed to load template '{template_ref}': {e}"
                    ))
                })?;
                let mut writer = SizeLimitedWriter::new(MAX_RENDERED_BYTES);
                tmpl.render_to_write(&ctx, &mut writer).map_err(|e| {
                    GatewayError::TemplateRender(format!(
                        "error rendering template '{template_ref}' for field '{field_name}' in profile '{}': {e}",
                        profile.name
                    ))
                })?;
                writer.into_string()?
            }
        };

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

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
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

        let result = render_profile(&profile, &templates, &payload, &[]).unwrap();
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

        let result = render_profile(&profile, &templates, &payload, &[]).unwrap();
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

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
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

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
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

        let err = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap_err();
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

        let err = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap_err();
        assert!(
            err.to_string().contains("error compiling")
                || err.to_string().contains("error rendering"),
            "expected compile/render error, got: {err}"
        );
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

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
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

        let result = render_profile(&profile, &templates, &payload, &[]).unwrap();
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
        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
        assert_eq!(result.fields.get("msg").unwrap(), "Hello ");
    }

    #[test]
    fn render_empty_profile() {
        let profile = make_profile("empty", HashMap::new());
        let payload = serde_json::json!({"foo": "bar"});

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
        assert!(result.fields.is_empty());
    }

    #[test]
    fn render_include_across_templates() {
        // Template A includes template B via {% include %}.
        let header = make_template("header", "<header>{{ title }}</header>");
        let page = make_template("page", "{% include 'header' %}<main>{{ body }}</main>");

        let mut templates = HashMap::new();
        templates.insert("header".to_string(), header);
        templates.insert("page".to_string(), page);

        let mut fields = HashMap::new();
        fields.insert(
            "html".to_string(),
            TemplateProfileField::Ref {
                template_ref: "page".to_string(),
            },
        );
        let profile = make_profile("include-test", fields);
        let payload = serde_json::json!({"title": "Welcome", "body": "Hello world"});

        let result = render_profile(&profile, &templates, &payload, &[]).unwrap();
        let html = result.fields.get("html").unwrap();
        assert!(html.contains("<header>Welcome</header>"));
        assert!(html.contains("<main>Hello world</main>"));
    }

    #[test]
    fn render_size_limit_aborts_during_render() {
        // Use a loop that would generate output exceeding MAX_RENDERED_BYTES.
        // We generate ~2 MB of output (2048 * 1024 = 2M chars).
        let mut fields = HashMap::new();
        fields.insert(
            "big".to_string(),
            TemplateProfileField::Inline(
                "{% for i in range(2048) %}{{ padding }}{% endfor %}".to_string(),
            ),
        );
        let profile = make_profile("big-output", fields);
        // Each iteration outputs 1024 'x' chars → 2048 * 1024 = 2 MB > 1 MB limit.
        let padding = "x".repeat(1024);
        let payload = serde_json::json!({"padding": padding});

        let err = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap_err();
        assert!(
            err.to_string().contains("size limit") || err.to_string().contains("error rendering"),
            "expected size-limit error, got: {err}"
        );
    }

    #[test]
    fn render_inline_via_named_template() {
        // Verify inline templates go through the named-template + render_to_write path.
        let mut fields = HashMap::new();
        fields.insert(
            "greeting".to_string(),
            TemplateProfileField::Inline("Hi {{ name }}, welcome!".to_string()),
        );
        let profile = make_profile("inline-named", fields);
        let payload = serde_json::json!({"name": "Dana"});

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
        assert_eq!(result.fields.get("greeting").unwrap(), "Hi Dana, welcome!");
    }

    fn make_attachment(id: &str, name: &str, filename: &str, content_type: &str) -> acteon_core::Attachment {
        acteon_core::Attachment {
            id: id.to_string(),
            name: name.to_string(),
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            data_base64: "dGVzdA==".to_string(), // "test"
        }
    }

    #[test]
    fn render_with_attachment_list() {
        let mut fields = HashMap::new();
        fields.insert(
            "files".to_string(),
            TemplateProfileField::Inline(
                "{% for att in attachments %}{{ att.filename }}{% if not loop.last %}, {% endif %}{% endfor %}".to_string(),
            ),
        );
        let profile = make_profile("att-list", fields);
        let payload = serde_json::json!({"subject": "Report"});
        let attachments = vec![
            make_attachment("a1", "Report PDF", "report.pdf", "application/pdf"),
            make_attachment("a2", "Logo", "logo.png", "image/png"),
        ];

        let result = render_profile(&profile, &HashMap::new(), &payload, &attachments).unwrap();
        assert_eq!(result.fields.get("files").unwrap(), "report.pdf, logo.png");
    }

    #[test]
    fn render_with_attachment_by_id() {
        let mut fields = HashMap::new();
        fields.insert(
            "msg".to_string(),
            TemplateProfileField::Inline(
                "Attached: {{ attachments_by_id.report.filename }} ({{ attachments_by_id.report.content_type }})".to_string(),
            ),
        );
        let profile = make_profile("att-by-id", fields);
        let payload = serde_json::json!({});
        let attachments = vec![
            make_attachment("report", "Quarterly Report", "q4.pdf", "application/pdf"),
        ];

        let result = render_profile(&profile, &HashMap::new(), &payload, &attachments).unwrap();
        assert_eq!(
            result.fields.get("msg").unwrap(),
            "Attached: q4.pdf (application/pdf)"
        );
    }

    #[test]
    fn render_attachments_count() {
        let mut fields = HashMap::new();
        fields.insert(
            "summary".to_string(),
            TemplateProfileField::Inline(
                "{{ attachments | length }} file(s) attached".to_string(),
            ),
        );
        let profile = make_profile("att-count", fields);
        let payload = serde_json::json!({});
        let attachments = vec![
            make_attachment("a1", "File 1", "f1.txt", "text/plain"),
            make_attachment("a2", "File 2", "f2.txt", "text/plain"),
            make_attachment("a3", "File 3", "f3.txt", "text/plain"),
        ];

        let result = render_profile(&profile, &HashMap::new(), &payload, &attachments).unwrap();
        assert_eq!(result.fields.get("summary").unwrap(), "3 file(s) attached");
    }

    #[test]
    fn render_empty_attachments() {
        let mut fields = HashMap::new();
        fields.insert(
            "msg".to_string(),
            TemplateProfileField::Inline(
                "{% if attachments %}Has files{% else %}No files{% endif %}".to_string(),
            ),
        );
        let profile = make_profile("no-att", fields);
        let payload = serde_json::json!({});

        let result = render_profile(&profile, &HashMap::new(), &payload, &[]).unwrap();
        assert_eq!(result.fields.get("msg").unwrap(), "No files");
    }

    #[test]
    fn render_attachment_metadata_excludes_data() {
        // Verify that data_base64 is NOT accessible in templates.
        let mut fields = HashMap::new();
        fields.insert(
            "data".to_string(),
            TemplateProfileField::Inline(
                "{{ attachments[0].data_base64 }}".to_string(),
            ),
        );
        let profile = make_profile("no-data", fields);
        let payload = serde_json::json!({});
        let attachments = vec![
            make_attachment("a1", "File", "f.txt", "text/plain"),
        ];

        let result = render_profile(&profile, &HashMap::new(), &payload, &attachments).unwrap();
        // data_base64 is not in context, so MiniJinja renders it as empty.
        assert_eq!(result.fields.get("data").unwrap(), "");
    }
}
