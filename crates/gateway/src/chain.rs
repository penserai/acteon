use acteon_core::Action;
use acteon_core::chain::{ChainStepConfig, StepResult};

/// Resolve template variables in a chain step's payload.
///
/// Supported variable patterns:
/// - `{{origin.payload.X}}`, `{{origin.namespace}}`, `{{origin.tenant}}`,
///   `{{origin.action_type}}`, `{{origin.metadata.X}}`
/// - `{{prev.body.X}}`, `{{prev.body}}` — previous step's response body
/// - `{{steps.NAME.body.X}}` — named step's response body
/// - `{{chain_id}}`, `{{step_index}}`
///
/// If the entire string value is a single `{{expr}}`, the original JSON type
/// from the referenced value is preserved. Otherwise, patterns are replaced
/// inline as strings. Missing paths resolve to `null`.
pub fn resolve_template(
    template: &serde_json::Value,
    origin: &Action,
    step_results: &[Option<StepResult>],
    steps_config: &[ChainStepConfig],
    chain_id: &str,
    step_index: usize,
) -> serde_json::Value {
    match template {
        serde_json::Value::String(s) => {
            resolve_string(s, origin, step_results, steps_config, chain_id, step_index)
        }
        serde_json::Value::Object(map) => {
            let mut result = serde_json::Map::new();
            for (k, v) in map {
                result.insert(
                    k.clone(),
                    resolve_template(v, origin, step_results, steps_config, chain_id, step_index),
                );
            }
            serde_json::Value::Object(result)
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|v| {
                    resolve_template(v, origin, step_results, steps_config, chain_id, step_index)
                })
                .collect(),
        ),
        // Primitives pass through unchanged.
        other => other.clone(),
    }
}

/// Resolve a single string value, handling both full-replacement and inline substitution.
fn resolve_string(
    s: &str,
    origin: &Action,
    step_results: &[Option<StepResult>],
    steps_config: &[ChainStepConfig],
    chain_id: &str,
    step_index: usize,
) -> serde_json::Value {
    let trimmed = s.trim();

    // If the entire string is a single {{expr}}, preserve the JSON type.
    if trimmed.starts_with("{{") && trimmed.ends_with("}}") && count_patterns(trimmed) == 1 {
        let expr = &trimmed[2..trimmed.len() - 2];
        return resolve_expr(
            expr,
            origin,
            step_results,
            steps_config,
            chain_id,
            step_index,
        );
    }

    // Otherwise, do inline string replacement for all {{...}} patterns.
    let mut result = s.to_string();
    while let Some(start) = result.find("{{") {
        let Some(end) = result[start..].find("}}") else {
            break;
        };
        let full_end = start + end + 2;
        let expr = &result[start + 2..start + end];
        let value = resolve_expr(
            expr,
            origin,
            step_results,
            steps_config,
            chain_id,
            step_index,
        );
        let replacement = json_to_inline_string(&value);
        result.replace_range(start..full_end, &replacement);
    }

    serde_json::Value::String(result)
}

/// Count the number of `{{...}}` patterns in a string.
fn count_patterns(s: &str) -> usize {
    let mut count = 0;
    let mut pos = 0;
    while let Some(start) = s[pos..].find("{{") {
        if let Some(end) = s[pos + start..].find("}}") {
            count += 1;
            pos = pos + start + end + 2;
        } else {
            break;
        }
    }
    count
}

/// Resolve a single expression like `origin.payload.query` or `prev.body.results`.
fn resolve_expr(
    expr: &str,
    origin: &Action,
    step_results: &[Option<StepResult>],
    steps_config: &[ChainStepConfig],
    chain_id: &str,
    step_index: usize,
) -> serde_json::Value {
    let parts: Vec<&str> = expr.split('.').collect();

    match parts.first().copied() {
        Some("origin") => resolve_origin(&parts[1..], origin),
        Some("prev") => resolve_prev(&parts[1..], step_results, step_index),
        Some("steps") => resolve_named_step(&parts[1..], step_results, steps_config),
        Some("chain_id") => serde_json::Value::String(chain_id.to_owned()),
        Some("step_index") => serde_json::json!(step_index),
        _ => serde_json::Value::Null,
    }
}

/// Resolve `origin.*` paths.
fn resolve_origin(path: &[&str], origin: &Action) -> serde_json::Value {
    match path.first().copied() {
        Some("payload") => navigate_json(&origin.payload, &path[1..]),
        Some("namespace") => serde_json::Value::String(origin.namespace.to_string()),
        Some("tenant") => serde_json::Value::String(origin.tenant.to_string()),
        Some("action_type") => serde_json::Value::String(origin.action_type.clone()),
        Some("provider") => serde_json::Value::String(origin.provider.to_string()),
        Some("id") => serde_json::Value::String(origin.id.to_string()),
        Some("metadata") => {
            if let Some(key) = path.get(1) {
                origin
                    .metadata
                    .labels
                    .get(*key)
                    .map_or(serde_json::Value::Null, |v| {
                        serde_json::Value::String(v.clone())
                    })
            } else {
                serde_json::to_value(&origin.metadata.labels).unwrap_or(serde_json::Value::Null)
            }
        }
        _ => serde_json::Value::Null,
    }
}

/// Resolve `prev.body.*` paths — the previous step's response body.
fn resolve_prev(
    path: &[&str],
    step_results: &[Option<StepResult>],
    step_index: usize,
) -> serde_json::Value {
    if step_index == 0 {
        return serde_json::Value::Null;
    }
    let prev_result = step_results.get(step_index - 1).and_then(|r| r.as_ref());

    match path.first().copied() {
        Some("body") => {
            if let Some(result) = prev_result {
                let body = result
                    .response_body
                    .clone()
                    .unwrap_or(serde_json::Value::Null);
                navigate_json(&body, &path[1..])
            } else {
                serde_json::Value::Null
            }
        }
        _ => serde_json::Value::Null,
    }
}

/// Resolve `steps.NAME.body.*` paths — a named step's response body.
fn resolve_named_step(
    path: &[&str],
    step_results: &[Option<StepResult>],
    steps_config: &[ChainStepConfig],
) -> serde_json::Value {
    let Some(name) = path.first().copied() else {
        return serde_json::Value::Null;
    };

    // Find the step index by name.
    let Some(idx) = steps_config.iter().position(|s| s.name == name) else {
        return serde_json::Value::Null;
    };

    let Some(Some(result)) = step_results.get(idx) else {
        return serde_json::Value::Null;
    };

    match path.get(1).copied() {
        Some("body") => {
            let body = result
                .response_body
                .clone()
                .unwrap_or(serde_json::Value::Null);
            navigate_json(&body, &path[2..])
        }
        _ => serde_json::Value::Null,
    }
}

/// Navigate into a JSON value by a dot-separated path.
fn navigate_json(value: &serde_json::Value, path: &[&str]) -> serde_json::Value {
    let mut current = value.clone();
    for &segment in path {
        match current {
            serde_json::Value::Object(ref map) => {
                current = map.get(segment).cloned().unwrap_or(serde_json::Value::Null);
            }
            serde_json::Value::Array(ref arr) => {
                if let Ok(idx) = segment.parse::<usize>() {
                    current = arr.get(idx).cloned().unwrap_or(serde_json::Value::Null);
                } else {
                    return serde_json::Value::Null;
                }
            }
            _ => return serde_json::Value::Null,
        }
    }
    current
}

/// Convert a JSON value to a string for inline template replacement.
fn json_to_inline_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_owned(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_origin() -> Action {
        Action::new(
            "ns",
            "tenant",
            "search-api",
            "web_search",
            serde_json::json!({
                "query": "rust async",
                "limit": 10,
                "nested": {"key": "deep_value"}
            }),
        )
    }

    fn test_steps_config() -> Vec<ChainStepConfig> {
        vec![
            ChainStepConfig::new("search", "search-api", "web_search", serde_json::json!({})),
            ChainStepConfig::new("summarize", "llm", "summarize", serde_json::json!({})),
            ChainStepConfig::new("email", "email", "send_email", serde_json::json!({})),
        ]
    }

    fn step_result(name: &str, body: serde_json::Value) -> Option<StepResult> {
        Some(StepResult {
            step_name: name.to_owned(),
            success: true,
            response_body: Some(body),
            error: None,
            completed_at: Utc::now(),
        })
    }

    #[test]
    fn resolve_origin_payload() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({"q": "{{origin.payload.query}}"});
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result, serde_json::json!({"q": "rust async"}));
    }

    #[test]
    fn resolve_origin_fields() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({
            "ns": "{{origin.namespace}}",
            "t": "{{origin.tenant}}",
            "at": "{{origin.action_type}}"
        });
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result["ns"], "ns");
        assert_eq!(result["t"], "tenant");
        assert_eq!(result["at"], "web_search");
    }

    #[test]
    fn resolve_prev_body() {
        let origin = test_origin();
        let steps = test_steps_config();
        let results = vec![step_result(
            "search",
            serde_json::json!({"results": "some data"}),
        )];
        let template = serde_json::json!({"text": "{{prev.body.results}}"});
        let result = resolve_template(&template, &origin, &results, &steps, "c1", 1);
        assert_eq!(result, serde_json::json!({"text": "some data"}));
    }

    #[test]
    fn resolve_prev_at_step_zero_is_null() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({"text": "{{prev.body}}"});
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result["text"], serde_json::Value::Null);
    }

    #[test]
    fn resolve_named_step() {
        let origin = test_origin();
        let steps = test_steps_config();
        let results = vec![
            step_result("search", serde_json::json!({"results": "search data"})),
            step_result("summarize", serde_json::json!({"summary": "brief"})),
        ];
        let template = serde_json::json!({"s": "{{steps.search.body.results}}"});
        let result = resolve_template(&template, &origin, &results, &steps, "c1", 2);
        assert_eq!(result, serde_json::json!({"s": "search data"}));
    }

    #[test]
    fn resolve_chain_id_and_step_index() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({
            "id": "{{chain_id}}",
            "idx": "{{step_index}}"
        });
        let result = resolve_template(&template, &origin, &[], &steps, "chain-42", 2);
        assert_eq!(result["id"], "chain-42");
        // step_index is a single {{expr}} so it preserves the JSON integer type.
        assert_eq!(result["idx"], serde_json::json!(2));
    }

    #[test]
    fn resolve_full_value_preserves_type() {
        let origin = test_origin();
        let steps = test_steps_config();
        // When the entire value is a single {{expr}} pointing to a number, preserve the type.
        let template = serde_json::json!({"limit": "{{origin.payload.limit}}"});
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result["limit"], serde_json::json!(10));
    }

    #[test]
    fn resolve_missing_path_returns_null() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({"x": "{{origin.payload.nonexistent}}"});
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result["x"], serde_json::Value::Null);
    }

    #[test]
    fn resolve_nested_json_path() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({"deep": "{{origin.payload.nested.key}}"});
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result["deep"], "deep_value");
    }

    #[test]
    fn resolve_inline_multiple_patterns() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!({
            "msg": "Search for {{origin.payload.query}} in {{origin.namespace}}"
        });
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result["msg"], "Search for rust async in ns");
    }

    #[test]
    fn resolve_array_templates() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!(["{{origin.payload.query}}", "literal"]);
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result[0], "rust async");
        assert_eq!(result[1], "literal");
    }

    #[test]
    fn resolve_passthrough_primitives() {
        let origin = test_origin();
        let steps = test_steps_config();
        let template = serde_json::json!(42);
        let result = resolve_template(&template, &origin, &[], &steps, "c1", 0);
        assert_eq!(result, serde_json::json!(42));
    }
}
