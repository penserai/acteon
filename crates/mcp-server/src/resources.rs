//! MCP resource handlers for Acteon.
//!
//! Resources provide read-only access to Acteon data such as
//! audit logs, rules, and event state.

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{AuditQuery, EventQuery};
use rmcp::{
    ErrorData as McpError,
    model::{
        AnnotateAble, ListResourcesResult, RawResource, ReadResourceRequestParams,
        ReadResourceResult, ResourceContents,
    },
};
use serde_json::json;

/// List the static top-level resources.
pub fn list_resources() -> ListResourcesResult {
    let resources = vec![
        RawResource::new("acteon://health", "Gateway health".to_string()).no_annotation(),
        RawResource::new("acteon://rules", "Active rules".to_string()).no_annotation(),
    ];

    ListResourcesResult {
        resources,
        next_cursor: None,
        meta: None,
    }
}

/// Read a specific resource by URI.
pub async fn read_resource(
    ops: &OpsClient,
    request: ReadResourceRequestParams,
) -> Result<ReadResourceResult, McpError> {
    let uri = request.uri.as_str();

    // Parse the URI: acteon://{kind}/{arg1}/{arg2?}
    let path = uri
        .strip_prefix("acteon://")
        .ok_or_else(|| McpError::invalid_params("URI must start with acteon://", None))?;

    let segments: Vec<&str> = path.splitn(3, '/').collect();

    match segments.first().copied() {
        Some("health") => read_health(ops, &request.uri).await,
        Some("rules") => read_rules(ops, &request.uri).await,
        Some("audit") => {
            let tenant = segments.get(1).copied().unwrap_or("default");
            read_audit(ops, tenant, &request.uri).await
        }
        Some("events") => {
            let tenant = segments.get(1).copied().unwrap_or("default");
            read_events(ops, tenant, &request.uri).await
        }
        _ => Err(McpError::resource_not_found(
            "unknown resource",
            Some(json!({"uri": uri})),
        )),
    }
}

async fn read_health(ops: &OpsClient, uri: &str) -> Result<ReadResourceResult, McpError> {
    let healthy = ops
        .client()
        .health()
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    let body = json!({ "healthy": healthy });
    Ok(ReadResourceResult {
        contents: vec![ResourceContents::text(
            serde_json::to_string_pretty(&body)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            uri,
        )],
    })
}

async fn read_rules(ops: &OpsClient, uri: &str) -> Result<ReadResourceResult, McpError> {
    let rules = ops
        .client()
        .list_rules()
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(ReadResourceResult {
        contents: vec![ResourceContents::text(
            serde_json::to_string_pretty(&rules)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            uri,
        )],
    })
}

async fn read_audit(
    ops: &OpsClient,
    tenant: &str,
    uri: &str,
) -> Result<ReadResourceResult, McpError> {
    let query = AuditQuery {
        tenant: Some(tenant.to_string()),
        limit: Some(25),
        ..AuditQuery::default()
    };

    let page = ops
        .client()
        .query_audit(&query)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(ReadResourceResult {
        contents: vec![ResourceContents::text(
            serde_json::to_string_pretty(&page)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            uri,
        )],
    })
}

async fn read_events(
    ops: &OpsClient,
    tenant: &str,
    uri: &str,
) -> Result<ReadResourceResult, McpError> {
    let query = EventQuery {
        namespace: "default".to_string(),
        tenant: tenant.to_string(),
        status: None,
        limit: Some(50),
    };

    let events = ops
        .client()
        .list_events(&query)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(ReadResourceResult {
        contents: vec![ResourceContents::text(
            serde_json::to_string_pretty(&events)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?,
            uri,
        )],
    })
}
