//! MCP server handler for Acteon.

use acteon_ops::OpsClient;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::router::{prompt::PromptRouter, tool::ToolRouter},
    model::{
        AnnotateAble, GetPromptRequestParams, GetPromptResult, Implementation, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
        RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
        ServerInfo,
    },
    prompt_handler,
    service::RequestContext,
    tool_handler,
};

use crate::resources;

/// The Acteon MCP Server.
///
/// Exposes Acteon gateway capabilities as MCP tools, resources, and prompts.
#[derive(Clone)]
pub struct ActeonMcpServer {
    pub(crate) ops: OpsClient,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl ActeonMcpServer {
    /// Create a new MCP server backed by the given operations client.
    pub fn new(ops: OpsClient) -> Self {
        let tool_router = Self::create_tool_router();
        let prompt_router = Self::create_prompt_router();
        Self {
            ops,
            tool_router,
            prompt_router,
        }
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for ActeonMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
            server_info: Implementation {
                name: "acteon-mcp-server".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                title: Some("Acteon MCP Server".into()),
                description: Some(
                    "Interact with the Acteon action gateway for dispatching, \
                     audit queries, rule management, and event operations."
                        .into(),
                ),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Acteon MCP Server — interact with the Acteon action gateway. \
                 Use the provided tools to dispatch actions, query the audit trail, \
                 manage events, list rules, and more. Use resources to read current \
                 state. Use prompts for guided operational workflows."
                    .to_string(),
            ),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(resources::list_resources())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        resources::read_resource(&self.ops, request).await
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            resource_templates: vec![
                RawResourceTemplate {
                    uri_template: "acteon://audit/{tenant}".into(),
                    name: "Audit trail".into(),
                    title: None,
                    description: Some("Recent audit records for a tenant".to_string()),
                    mime_type: Some("application/json".into()),
                    icons: None,
                }
                .no_annotation(),
                RawResourceTemplate {
                    uri_template: "acteon://rules/{tenant}".into(),
                    name: "Active rules".into(),
                    title: None,
                    description: Some("The active rule set for a tenant".to_string()),
                    mime_type: Some("application/json".into()),
                    icons: None,
                }
                .no_annotation(),
                RawResourceTemplate {
                    uri_template: "acteon://events/{tenant}".into(),
                    name: "Stateful events".into(),
                    title: None,
                    description: Some("Open stateful events for a tenant".to_string()),
                    mime_type: Some("application/json".into()),
                    icons: None,
                }
                .no_annotation(),
            ],
            next_cursor: None,
            meta: None,
        })
    }
}
