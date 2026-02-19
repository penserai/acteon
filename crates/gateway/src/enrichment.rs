use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Arc;
use std::time::{Duration, Instant};

use acteon_core::Action;
use acteon_core::enrichment::{EnrichmentConfig, EnrichmentFailurePolicy, EnrichmentOutcome};
use acteon_provider::ResourceLookup;
use tracing::{debug, warn};

use crate::error::GatewayError;

/// Resolve placeholders in an enrichment parameter template against an action.
///
/// Supported placeholders:
/// - `{{payload.field_name}}` — value from the action payload (dot-separated for nested paths)
/// - `{{namespace}}` — the action namespace
/// - `{{tenant}}` — the action tenant
/// - `{{action_type}}` — the action type
///
/// When a string value is **exactly** a single placeholder (e.g., `"{{payload.names}}"`),
/// the original JSON value is preserved (arrays, objects, etc.). When a placeholder
/// appears among other text, string interpolation is performed. Missing fields
/// resolve to `null`.
#[must_use]
pub fn resolve_enrichment_params(
    template: &serde_json::Value,
    action: &Action,
) -> serde_json::Value {
    match template {
        serde_json::Value::String(s) => resolve_string_template(s, action),
        serde_json::Value::Array(arr) => {
            let resolved: Vec<serde_json::Value> = arr
                .iter()
                .map(|v| resolve_enrichment_params(v, action))
                .collect();
            serde_json::Value::Array(resolved)
        }
        serde_json::Value::Object(map) => {
            let resolved: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), resolve_enrichment_params(v, action)))
                .collect();
            serde_json::Value::Object(resolved)
        }
        // Numbers, bools, null pass through unchanged.
        other => other.clone(),
    }
}

/// Resolve a single string template value.
fn resolve_string_template(s: &str, action: &Action) -> serde_json::Value {
    // Fast path: if the entire string is a single placeholder, preserve the original type.
    if let Some(inner) = extract_sole_placeholder(s) {
        return resolve_placeholder(&inner, action);
    }

    // Otherwise, do string interpolation for any embedded placeholders.
    let mut result = s.to_owned();
    let mut start = 0;
    while let Some(open) = result[start..].find("{{") {
        let abs_open = start + open;
        if let Some(close) = result[abs_open..].find("}}") {
            let abs_close = abs_open + close;
            let placeholder = &result[abs_open + 2..abs_close].trim().to_owned();
            let value = resolve_placeholder(placeholder, action);
            let replacement = match &value {
                serde_json::Value::String(v) => v.clone(),
                serde_json::Value::Null => "null".to_owned(),
                other => other.to_string(),
            };
            result.replace_range(abs_open..abs_close + 2, &replacement);
            start = abs_open + replacement.len();
        } else {
            break;
        }
    }

    serde_json::Value::String(result)
}

/// If the string is exactly `{{ ... }}` (one placeholder, nothing else), return
/// the inner key. Trims whitespace inside the braces.
fn extract_sole_placeholder(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.starts_with("{{") && trimmed.ends_with("}}") {
        let inner = &trimmed[2..trimmed.len() - 2];
        // Make sure there are no other `{{` or `}}` inside.
        if !inner.contains("{{") && !inner.contains("}}") {
            return Some(inner.trim().to_owned());
        }
    }
    None
}

/// Resolve a single placeholder key to a JSON value.
fn resolve_placeholder(key: &str, action: &Action) -> serde_json::Value {
    match key {
        "namespace" => serde_json::Value::String(action.namespace.to_string()),
        "tenant" => serde_json::Value::String(action.tenant.to_string()),
        "action_type" => serde_json::Value::String(action.action_type.clone()),
        k if k.starts_with("payload.") => {
            let path = &k["payload.".len()..];
            resolve_payload_path(&action.payload, path)
        }
        _ => serde_json::Value::Null,
    }
}

/// Walk a dot-separated path into a JSON value.
fn resolve_payload_path(value: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = value;
    for segment in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(segment) {
                    current = v;
                } else {
                    return serde_json::Value::Null;
                }
            }
            _ => return serde_json::Value::Null,
        }
    }
    current.clone()
}

/// Apply matching enrichment configs to an action, merging lookup results into
/// the payload.
///
/// Returns the diagnostic outcomes for each enrichment that was attempted
/// (skipped enrichments are not included).
pub async fn apply_enrichments<S: BuildHasher>(
    action: &mut Action,
    enrichments: &[EnrichmentConfig],
    resource_lookups: &HashMap<String, Arc<dyn ResourceLookup>, S>,
) -> Result<Vec<EnrichmentOutcome>, GatewayError> {
    let mut outcomes = Vec::new();

    for config in enrichments {
        // Check filter criteria.
        if !matches_enrichment_filter(action, config) {
            debug!(
                enrichment = %config.name,
                "enrichment skipped: action does not match filter"
            );
            continue;
        }

        // Find the lookup provider.
        let Some(lookup) = resource_lookups.get(&config.lookup_provider) else {
            let msg = format!(
                "enrichment '{}': lookup provider '{}' not found",
                config.name, config.lookup_provider
            );
            match config.failure_policy {
                EnrichmentFailurePolicy::FailOpen => {
                    warn!("{msg}");
                    outcomes.push(EnrichmentOutcome {
                        name: config.name.clone(),
                        provider: config.lookup_provider.clone(),
                        resource_type: config.resource_type.clone(),
                        success: false,
                        error: Some(msg),
                        duration_ms: 0,
                    });
                    continue;
                }
                EnrichmentFailurePolicy::FailClosed => {
                    return Err(GatewayError::Enrichment(msg));
                }
            }
        };
        let lookup = Arc::clone(lookup);

        // Resolve template parameters.
        let resolved_params = resolve_enrichment_params(&config.params, action);

        // Perform the lookup with a timeout.
        let timeout = Duration::from_secs(config.timeout_seconds);
        let start = Instant::now();
        let lookup_result = tokio::time::timeout(
            timeout,
            lookup.lookup(&config.resource_type, &resolved_params),
        )
        .await;
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        match lookup_result {
            Ok(Ok(data)) => {
                debug!(
                    enrichment = %config.name,
                    merge_key = %config.merge_key,
                    elapsed_ms,
                    "enrichment lookup succeeded"
                );
                // Merge the result into the payload.
                if let Some(obj) = action.payload.as_object_mut() {
                    obj.insert(config.merge_key.clone(), data);
                }
                outcomes.push(EnrichmentOutcome {
                    name: config.name.clone(),
                    provider: config.lookup_provider.clone(),
                    resource_type: config.resource_type.clone(),
                    success: true,
                    error: None,
                    duration_ms: elapsed_ms,
                });
            }
            Ok(Err(e)) => {
                let msg = format!("enrichment '{}': lookup failed: {e}", config.name);
                handle_enrichment_failure(config, &msg, elapsed_ms, &mut outcomes)?;
            }
            Err(_) => {
                let msg = format!(
                    "enrichment '{}': lookup timed out after {timeout:?}",
                    config.name
                );
                handle_enrichment_failure(config, &msg, elapsed_ms, &mut outcomes)?;
            }
        }
    }

    Ok(outcomes)
}

/// Check whether an action matches the filter criteria of an enrichment config.
fn matches_enrichment_filter(action: &Action, config: &EnrichmentConfig) -> bool {
    if let Some(ref ns) = config.namespace
        && action.namespace.as_str() != ns
    {
        return false;
    }
    if let Some(ref t) = config.tenant
        && action.tenant.as_str() != t
    {
        return false;
    }
    if let Some(ref at) = config.action_type
        && action.action_type != *at
    {
        return false;
    }
    if let Some(ref p) = config.provider
        && action.provider.as_str() != p
    {
        return false;
    }
    true
}

/// Handle a failed enrichment lookup according to the failure policy.
fn handle_enrichment_failure(
    config: &EnrichmentConfig,
    msg: &str,
    elapsed_ms: u64,
    outcomes: &mut Vec<EnrichmentOutcome>,
) -> Result<(), GatewayError> {
    match config.failure_policy {
        EnrichmentFailurePolicy::FailOpen => {
            warn!("{msg}");
            outcomes.push(EnrichmentOutcome {
                name: config.name.clone(),
                provider: config.lookup_provider.clone(),
                resource_type: config.resource_type.clone(),
                success: false,
                error: Some(msg.to_owned()),
                duration_ms: elapsed_ms,
            });
            Ok(())
        }
        EnrichmentFailurePolicy::FailClosed => Err(GatewayError::Enrichment(msg.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_provider::ProviderError;
    use async_trait::async_trait;

    // -----------------------------------------------------------------------
    // Mock providers
    // -----------------------------------------------------------------------

    struct MockResourceLookup {
        response: serde_json::Value,
    }

    #[async_trait]
    impl ResourceLookup for MockResourceLookup {
        async fn lookup(
            &self,
            _resource_type: &str,
            _params: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            Ok(self.response.clone())
        }

        fn supported_resource_types(&self) -> Vec<String> {
            vec!["test".to_owned()]
        }
    }

    struct FailingResourceLookup;

    #[async_trait]
    impl ResourceLookup for FailingResourceLookup {
        async fn lookup(
            &self,
            _resource_type: &str,
            _params: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            Err(ProviderError::Connection(
                "lookup connection refused".into(),
            ))
        }

        fn supported_resource_types(&self) -> Vec<String> {
            vec!["test".to_owned()]
        }
    }

    // -----------------------------------------------------------------------
    // Helper
    // -----------------------------------------------------------------------

    fn test_action() -> Action {
        Action::new(
            "infra",
            "tenant-1",
            "aws-autoscaling",
            "set_desired_capacity",
            serde_json::json!({
                "asg_name": "my-asg",
                "desired_capacity": 5,
                "names": ["asg-a", "asg-b"],
                "nested": {
                    "key": "deep-value"
                }
            }),
        )
    }

    fn base_enrichment_config() -> EnrichmentConfig {
        EnrichmentConfig {
            name: "test-enrichment".into(),
            namespace: None,
            tenant: None,
            action_type: None,
            provider: None,
            lookup_provider: "mock".into(),
            resource_type: "test".into(),
            params: serde_json::json!({}),
            merge_key: "enriched".into(),
            timeout_seconds: 5,
            failure_policy: EnrichmentFailurePolicy::FailOpen,
        }
    }

    // -----------------------------------------------------------------------
    // Template resolution tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_payload_field() {
        let action = test_action();
        let template = serde_json::json!({
            "name": "{{payload.asg_name}}"
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert_eq!(resolved["name"], "my-asg");
    }

    #[test]
    fn test_resolve_full_value_preserves_type() {
        let action = test_action();
        // The entire string is a single placeholder pointing to an array.
        let template = serde_json::json!({
            "names": "{{payload.names}}"
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert!(resolved["names"].is_array());
        let arr = resolved["names"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], "asg-a");
        assert_eq!(arr[1], "asg-b");
    }

    #[test]
    fn test_resolve_missing_field() {
        let action = test_action();
        let template = serde_json::json!({
            "missing": "{{payload.nonexistent}}"
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert!(resolved["missing"].is_null());
    }

    #[test]
    fn test_resolve_namespace_tenant() {
        let action = test_action();
        let template = serde_json::json!({
            "ns": "{{namespace}}",
            "t": "{{tenant}}",
            "at": "{{action_type}}"
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert_eq!(resolved["ns"], "infra");
        assert_eq!(resolved["t"], "tenant-1");
        assert_eq!(resolved["at"], "set_desired_capacity");
    }

    #[test]
    fn test_resolve_nested_template() {
        let action = test_action();
        let template = serde_json::json!({
            "outer": {
                "inner_name": "{{payload.asg_name}}",
                "inner_ns": "{{namespace}}"
            },
            "list": ["{{payload.asg_name}}", "static"]
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert_eq!(resolved["outer"]["inner_name"], "my-asg");
        assert_eq!(resolved["outer"]["inner_ns"], "infra");
        assert_eq!(resolved["list"][0], "my-asg");
        assert_eq!(resolved["list"][1], "static");
    }

    #[test]
    fn test_resolve_nested_payload_path() {
        let action = test_action();
        let template = serde_json::json!({
            "deep": "{{payload.nested.key}}"
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert_eq!(resolved["deep"], "deep-value");
    }

    #[test]
    fn test_resolve_interpolation_in_longer_string() {
        let action = test_action();
        let template = serde_json::json!({
            "msg": "ASG {{payload.asg_name}} in {{namespace}}"
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert_eq!(resolved["msg"], "ASG my-asg in infra");
    }

    #[test]
    fn test_resolve_non_string_passthrough() {
        let action = test_action();
        let template = serde_json::json!({
            "count": 42,
            "flag": true,
            "nothing": null
        });
        let resolved = resolve_enrichment_params(&template, &action);
        assert_eq!(resolved["count"], 42);
        assert_eq!(resolved["flag"], true);
        assert!(resolved["nothing"].is_null());
    }

    // -----------------------------------------------------------------------
    // apply_enrichments tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_enrichment_merges_data() {
        let mut action = test_action();
        let mock = Arc::new(MockResourceLookup {
            response: serde_json::json!({
                "auto_scaling_groups": [{
                    "auto_scaling_group_name": "my-asg",
                    "desired_capacity": 3
                }]
            }),
        }) as Arc<dyn ResourceLookup>;

        let mut lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();
        lookups.insert("mock".into(), mock);

        let config = base_enrichment_config();

        let outcomes = apply_enrichments(&mut action, &[config], &lookups)
            .await
            .unwrap();

        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert!(outcomes[0].error.is_none());

        // Verify the data was merged under the merge_key.
        let enriched = &action.payload["enriched"];
        assert!(enriched.is_object());
        assert!(enriched["auto_scaling_groups"].is_array());
    }

    #[tokio::test]
    async fn test_enrichment_skips_non_matching() {
        let mut action = test_action();
        let mock = Arc::new(MockResourceLookup {
            response: serde_json::json!({"data": true}),
        }) as Arc<dyn ResourceLookup>;

        let mut lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();
        lookups.insert("mock".into(), mock);

        let mut config = base_enrichment_config();
        // Set an action_type filter that won't match.
        config.action_type = Some("describe_instances".into());

        let outcomes = apply_enrichments(&mut action, &[config], &lookups)
            .await
            .unwrap();

        // The enrichment was skipped, not attempted.
        assert!(outcomes.is_empty());
        // Payload should not have the merge key.
        assert!(action.payload.get("enriched").is_none());
    }

    #[tokio::test]
    async fn test_enrichment_fail_open() {
        let mut action = test_action();
        let failing = Arc::new(FailingResourceLookup) as Arc<dyn ResourceLookup>;

        let mut lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();
        lookups.insert("mock".into(), failing);

        let mut config = base_enrichment_config();
        config.failure_policy = EnrichmentFailurePolicy::FailOpen;

        let outcomes = apply_enrichments(&mut action, &[config], &lookups)
            .await
            .unwrap();

        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].success);
        assert!(outcomes[0].error.is_some());
        // Payload should not have the merge key since lookup failed.
        assert!(action.payload.get("enriched").is_none());
    }

    #[tokio::test]
    async fn test_enrichment_fail_closed() {
        let mut action = test_action();
        let failing = Arc::new(FailingResourceLookup) as Arc<dyn ResourceLookup>;

        let mut lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();
        lookups.insert("mock".into(), failing);

        let mut config = base_enrichment_config();
        config.failure_policy = EnrichmentFailurePolicy::FailClosed;

        let result = apply_enrichments(&mut action, &[config], &lookups).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GatewayError::Enrichment(_)));
        assert!(err.to_string().contains("lookup failed"));
    }

    #[tokio::test]
    async fn test_enrichment_provider_not_found_fail_open() {
        let mut action = test_action();
        let lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();

        let mut config = base_enrichment_config();
        config.lookup_provider = "nonexistent".into();
        config.failure_policy = EnrichmentFailurePolicy::FailOpen;

        let outcomes = apply_enrichments(&mut action, &[config], &lookups)
            .await
            .unwrap();

        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].success);
        assert!(outcomes[0].error.as_ref().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_enrichment_provider_not_found_fail_closed() {
        let mut action = test_action();
        let lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();

        let mut config = base_enrichment_config();
        config.lookup_provider = "nonexistent".into();
        config.failure_policy = EnrichmentFailurePolicy::FailClosed;

        let result = apply_enrichments(&mut action, &[config], &lookups).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GatewayError::Enrichment(_)));
    }

    #[tokio::test]
    async fn test_enrichment_multiple_configs() {
        let mut action = test_action();
        let mock = Arc::new(MockResourceLookup {
            response: serde_json::json!({"status": "ok"}),
        }) as Arc<dyn ResourceLookup>;

        let mut lookups: HashMap<String, Arc<dyn ResourceLookup>> = HashMap::new();
        lookups.insert("mock".into(), mock);

        let mut config1 = base_enrichment_config();
        config1.name = "enrichment-1".into();
        config1.merge_key = "data_1".into();

        let mut config2 = base_enrichment_config();
        config2.name = "enrichment-2".into();
        config2.merge_key = "data_2".into();

        let outcomes = apply_enrichments(&mut action, &[config1, config2], &lookups)
            .await
            .unwrap();

        assert_eq!(outcomes.len(), 2);
        assert!(outcomes[0].success);
        assert!(outcomes[1].success);
        assert!(action.payload.get("data_1").is_some());
        assert!(action.payload.get("data_2").is_some());
    }

    #[test]
    fn test_matches_enrichment_filter_all_none() {
        let action = test_action();
        let config = base_enrichment_config();
        assert!(matches_enrichment_filter(&action, &config));
    }

    #[test]
    fn test_matches_enrichment_filter_namespace_match() {
        let action = test_action();
        let mut config = base_enrichment_config();
        config.namespace = Some("infra".into());
        assert!(matches_enrichment_filter(&action, &config));
    }

    #[test]
    fn test_matches_enrichment_filter_namespace_mismatch() {
        let action = test_action();
        let mut config = base_enrichment_config();
        config.namespace = Some("other".into());
        assert!(!matches_enrichment_filter(&action, &config));
    }

    #[test]
    fn test_matches_enrichment_filter_all_criteria() {
        let action = test_action();
        let mut config = base_enrichment_config();
        config.namespace = Some("infra".into());
        config.tenant = Some("tenant-1".into());
        config.action_type = Some("set_desired_capacity".into());
        config.provider = Some("aws-autoscaling".into());
        assert!(matches_enrichment_filter(&action, &config));
    }
}
