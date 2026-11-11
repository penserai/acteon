//! MCP server handler for Acteon.

use acteon_ops::OpsClient;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::router::{prompt::PromptRouter, tool::ToolRouter},
    model::{
        Implementation, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ResourceTemplate,
        ServerCapabilities, ServerConfig,
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

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
#[allow(unknown_lints)] // The async trait lint was introduced after our Rust 1.88 MSRV.
#[allow(
    clippy::unused_async_trait_impl,
    reason = "rmcp handler macros generate async methods for synchronous listings"
)]
impl ServerHandler for ActeonMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().enable_prompts().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_server_info(Implementation::new("acteon-mcp-server", env!("CARGO_PKG_VERSION"))
                .with_title("Acteon MCP Server")
                .with_description("Interact with the Acteon action gateway for dispatching, audit queries, rule management, and event operations."))
            .with_instructions("Acteon MCP Server — interact with the Acteon action gateway. Use the provided tools to dispatch actions, query the audit trail, manage events, list rules, and more. Use resources to read current state. Use prompts for guided operational workflows.")
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, McpError>> + Send {
        std::future::ready(Ok(resources::list_resources()))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        resources::read_resource(&self.ops, request)
            .await
            .map(ReadResourceResponse::from)
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send
    {
        std::future::ready(Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("acteon://audit", "Audit trail")
                .with_description("Recent audit records in the authenticated context")
                .with_mime_type("application/json"),
            ResourceTemplate::new("acteon://rules", "Active rules")
                .with_description("The active rule set in the authenticated context")
                .with_mime_type("application/json"),
            ResourceTemplate::new("acteon://events", "Stateful events")
                .with_description("Open stateful events in the authenticated context")
                .with_mime_type("application/json"),
        ])))
    }
}
