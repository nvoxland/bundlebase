//! MCP server implementation for bundlebase.
//!
//! Provides an MCP server over stdio that exposes bundlebase tools to AI assistants.
//! Can start with or without a bundle — use `open_bundle` or `create_bundle` to load one.

use bundlebase::{Bundle, BundleBuilder, BundleFacade, BundlebaseError};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::schemars;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

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

/// Parameter struct for bundle lifecycle tools.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BundlePathParams {
    /// Path or URL to the bundle
    #[schemars(description = "Path or URL to the bundle (e.g., './my-bundle', 's3://bucket/bundle')")]
    pub path: String,
}

/// Parameter struct for `open_bundle` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenBundleParams {
    /// Path or URL to the bundle
    #[schemars(description = "Path or URL to the bundle (e.g., './my-bundle', 's3://bucket/bundle')")]
    pub path: String,

    /// Open in read-only mode (default: false)
    #[schemars(description = "Open in read-only mode (default: false). When true, only SELECT and EXPLAIN are allowed.")]
    pub read_only: Option<bool>,
}

const NO_BUNDLE_MSG: &str = "No bundle is open. Use the create_bundle or open_bundle tool first.";

/// MCP server for bundlebase bundles.
#[derive(Clone)]
pub struct BundlebaseMcpServer {
    bundle: Arc<Mutex<Option<Arc<dyn BundleFacade>>>>,
    tool_router: ToolRouter<Self>,
}

impl BundlebaseMcpServer {
    /// Get the current bundle, returning an error message if none is open.
    async fn get_bundle(&self) -> Result<Arc<dyn BundleFacade>, String> {
        self.bundle
            .lock()
            .await
            .clone()
            .ok_or_else(|| NO_BUNDLE_MSG.to_string())
    }
}

#[tool_router]
impl BundlebaseMcpServer {
    pub fn new(bundle: Option<Arc<dyn BundleFacade>>) -> Self {
        Self {
            bundle: Arc::new(Mutex::new(bundle)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "create_bundle",
        description = "Create a new bundle at the given path. Must be called before using other tools if no bundle is open."
    )]
    async fn create_bundle(
        &self,
        Parameters(params): Parameters<BundlePathParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let guard = self.bundle.lock().await;
            if guard.is_some() {
                return Ok(CallToolResult::error(vec![Content::text(
                    "A bundle is already open. Use close_bundle first.",
                )]));
            }
        }

        match BundleBuilder::create(&params.path, None).await {
            Ok(builder) => {
                let url = builder.url().to_string();
                *self.bundle.lock().await = Some(builder);
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Created bundle at {}",
                    url
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to create bundle: {}",
                e
            ))])),
        }
    }

    #[tool(
        name = "open_bundle",
        description = "Open an existing bundle at the given path. Opens in read-write mode by default. Must be called before using other tools if no bundle is open."
    )]
    async fn open_bundle(
        &self,
        Parameters(params): Parameters<OpenBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let guard = self.bundle.lock().await;
            if guard.is_some() {
                return Ok(CallToolResult::error(vec![Content::text(
                    "A bundle is already open. Use close_bundle first.",
                )]));
            }
        }

        let read_only = params.read_only.unwrap_or(false);
        let result: Result<Arc<dyn BundleFacade>, BundlebaseError> = if read_only {
            Bundle::open(&params.path, None)
                .await
                .map(|b| b as Arc<dyn BundleFacade>)
        } else {
            match Bundle::open(&params.path, None).await {
                Ok(b) => b.extend(None).await.map(|b| b as Arc<dyn BundleFacade>),
                Err(e) => Err(e),
            }
        };

        match result {
            Ok(bundle) => {
                let url = bundle.url().to_string();
                let version = bundle.version();
                let commits = bundle.history().len();
                *self.bundle.lock().await = Some(bundle);
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Opened bundle at {} (version {}, {} commit{})",
                    url,
                    version,
                    commits,
                    if commits == 1 { "" } else { "s" }
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to open bundle: {}",
                e
            ))])),
        }
    }

    #[tool(
        name = "close_bundle",
        description = "Close the currently open bundle. Required before opening or creating a different bundle."
    )]
    async fn close_bundle(&self) -> Result<CallToolResult, McpError> {
        let mut guard = self.bundle.lock().await;
        if guard.is_none() {
            return Ok(CallToolResult::error(vec![Content::text(
                "No bundle is open.",
            )]));
        }
        *guard = None;
        Ok(CallToolResult::success(vec![Content::text(
            "Bundle closed.",
        )]))
    }

    #[tool(
        name = "query",
        description = "Execute a SQL query or bundlebase command against the open bundle. Supports SELECT queries, ATTACH/DETACH for data sources, FILTER/SELECT for transformations, COMMIT for saving changes, and all other bundlebase SQL extensions. Results are returned as JSON, limited to 1000 rows."
    )]
    async fn query(
        &self,
        Parameters(params): Parameters<QueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::execute_query(&bundle, &params.sql).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "schema",
        description = "Get the open bundle's column schema including column names, data types, and nullability."
    )]
    async fn schema(&self) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::get_schema(&bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(name = "count", description = "Get the total number of rows in the open bundle.")]
    async fn count(&self) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::get_count(&bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "sample",
        description = "Get a sample of rows from the open bundle. Returns up to the specified number of rows (default 10, max 1000) as JSON."
    )]
    async fn sample(
        &self,
        Parameters(params): Parameters<SampleParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        let limit = params.limit.unwrap_or(10).min(1000);
        match tools::get_sample(&bundle, limit).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "status",
        description = "Get the open bundle's status including any uncommitted changes."
    )]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::get_status(&bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "history",
        description = "Get the open bundle's commit history showing past changes."
    )]
    async fn history(&self) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::get_history(&bundle).await {
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
                "Bundlebase MCP server for versioned, queryable data bundles. \
                 Start by calling 'create_bundle' or 'open_bundle' to load a bundle, then use \
                 'query' to execute SQL, 'schema'/'sample'/'count' to explore data, \
                 'status'/'history' for version info. Call 'close_bundle' to switch bundles.",
            )
    }
}

/// Start the MCP server over stdio.
///
/// If `bundle` is Some, starts with that bundle pre-opened.
/// If None, starts empty — agent must call create_bundle or open_bundle first.
pub async fn start(bundle: Option<Arc<dyn BundleFacade>>) -> Result<(), BundlebaseError> {
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
    async fn test_server_instantiation_with_bundle() {
        let bundle = create_test_bundle().await;
        let server = BundlebaseMcpServer::new(Some(bundle));
        let info = server.get_info();
        assert_eq!(info.server_info.name, "bundlebase");
        assert!(info.instructions.unwrap_or_default().contains("create_bundle"));
    }

    #[tokio::test]
    async fn test_server_instantiation_without_bundle() {
        let server = BundlebaseMcpServer::new(None);
        assert!(server.get_bundle().await.is_err());
    }

    #[tokio::test]
    async fn test_get_bundle_returns_error_when_none() {
        let server = BundlebaseMcpServer::new(None);
        match server.get_bundle().await {
            Ok(_) => panic!("Expected error when no bundle is open"),
            Err(err) => {
                assert!(err.contains("No bundle is open"));
                assert!(err.contains("create_bundle"));
            }
        }
    }
}
