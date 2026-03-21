//! MCP server implementation for bundlebase.
//!
//! Provides an MCP server over stdio that exposes bundlebase tools to AI assistants.

use bundlebase::{BundlebaseError, BundleFacade};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::schemars;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use serde::Deserialize;
use std::sync::Arc;

use super::tools;

/// Parameter struct for the `query` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// SQL query or bundlebase command to execute (e.g., "SELECT * FROM bundle",
    /// "ATTACH 'data.csv'", "FILTER WHERE x > 5", "COMMIT 'message'")
    #[schemars(description = "SQL query or bundlebase command to execute")]
    pub sql: String,
}

/// Parameter struct for the `sample` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SampleParams {
    /// Number of rows to return (default: 10)
    #[schemars(description = "Number of sample rows to return (default: 10, max: 1000)")]
    pub limit: Option<usize>,
}

/// MCP server for bundlebase bundles.
#[derive(Clone)]
pub struct BundlebaseMcpServer {
    bundle: Arc<dyn BundleFacade>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl BundlebaseMcpServer {
    pub fn new(bundle: Arc<dyn BundleFacade>) -> Self {
        Self {
            bundle,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "query",
        description = "Execute a SQL query or bundlebase command against the bundle. Supports SELECT queries, ATTACH/DETACH for data sources, FILTER/SELECT for transformations, COMMIT for saving changes, and all other bundlebase SQL extensions. Results are returned as JSON, limited to 1000 rows."
    )]
    async fn query(
        &self,
        Parameters(params): Parameters<QueryParams>,
    ) -> Result<CallToolResult, McpError> {
        match tools::execute_query(&self.bundle, &params.sql).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "schema",
        description = "Get the bundle's column schema including column names, data types, and nullability."
    )]
    async fn schema(&self) -> Result<CallToolResult, McpError> {
        match tools::get_schema(&self.bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(name = "count", description = "Get the total number of rows in the bundle.")]
    async fn count(&self) -> Result<CallToolResult, McpError> {
        match tools::get_count(&self.bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "sample",
        description = "Get a sample of rows from the bundle. Returns up to the specified number of rows (default 10, max 1000) as JSON."
    )]
    async fn sample(
        &self,
        Parameters(params): Parameters<SampleParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(10).min(1000);
        match tools::get_sample(&self.bundle, limit).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "status",
        description = "Get the current bundle status including any uncommitted changes."
    )]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        match tools::get_status(&self.bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "history",
        description = "Get the bundle's commit history showing past changes."
    )]
    async fn history(&self) -> Result<CallToolResult, McpError> {
        match tools::get_history(&self.bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }
}

#[tool_handler]
impl ServerHandler for BundlebaseMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "bundlebase",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Bundlebase MCP server. Use the 'query' tool to execute SQL queries and bundlebase commands. \
                 Use 'schema' to understand the data structure, 'sample' to preview data, \
                 'count' for row counts, 'status' for uncommitted changes, and 'history' for commit log.",
            )
    }
}

/// Start the MCP server over stdio.
pub async fn start(bundle: Arc<dyn BundleFacade>) -> Result<(), BundlebaseError> {
    let server = BundlebaseMcpServer::new(bundle);

    let service = server
        .serve(rmcp::transport::io::stdio())
        .await
        .map_err(|e| BundlebaseError::from(format!("MCP server error: {}", e)))?;

    service
        .waiting()
        .await
        .map_err(|e| BundlebaseError::from(format!("MCP server error: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase::BundleBuilder;

    async fn create_test_bundle() -> Arc<dyn BundleFacade> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("System time before UNIX epoch")
            .as_nanos();
        BundleBuilder::create(&format!("memory:///mcp_server_test_{}", ts), None)
            .await
            .expect("Failed to create test bundle")
    }

    #[tokio::test]
    async fn test_server_instantiation() {
        let bundle = create_test_bundle().await;
        let server = BundlebaseMcpServer::new(bundle);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "bundlebase");
        assert!(info.instructions.is_some());
    }
}
