//! MCP server implementation for bundlebase.
//!
//! Provides an MCP server over stdio that exposes bundlebase tools to AI assistants.
//! Can start with or without a bundle — use `open_bundle` or `create_bundle` to load one.

use bundlebase::{Bundle, BundleBuilder, BundleFacade};
use bundlebase_common::BundlebaseError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::schemars;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::tools;

/// Parameter struct for the `query` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// Identifier of the open bundle to query
    #[schemars(description = "Bundle identifier (as provided when opening/creating)")]
    pub bundle: String,

    /// SQL query or bundlebase command to execute (e.g., "SELECT * FROM bundle",
    /// "ATTACH 'data.csv'", "FILTER WHERE x > 5", "COMMIT 'message'")
    #[schemars(description = "SQL query or bundlebase command to execute")]
    pub sql: String,
}

/// Parameter struct for the `sample` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SampleParams {
    /// Identifier of the open bundle to sample
    #[schemars(description = "Bundle identifier (as provided when opening/creating)")]
    pub bundle: String,

    /// Number of rows to return (default: 10)
    #[schemars(description = "Number of sample rows to return (default: 10, max: 1000)")]
    pub limit: Option<usize>,
}

/// Parameter struct for bundle lifecycle tools that create/open a bundle.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BundlePathParams {
    /// Unique identifier for this bundle in future tool calls
    #[schemars(description = "Unique identifier for this bundle (used to reference it in other tools)")]
    pub bundle: String,

    /// Path or URL to the bundle
    #[schemars(description = "Path or URL to the bundle (e.g., './my-bundle', 's3://bucket/bundle')")]
    pub path: String,
}

/// Parameter struct for `open_bundle` tool.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenBundleParams {
    /// Unique identifier for this bundle in future tool calls
    #[schemars(description = "Unique identifier for this bundle (used to reference it in other tools)")]
    pub bundle: String,

    /// Path or URL to the bundle
    #[schemars(description = "Path or URL to the bundle (e.g., './my-bundle', 's3://bucket/bundle')")]
    pub path: String,

    /// Open in read-only mode (default: false)
    #[schemars(description = "Open in read-only mode (default: false). When true, only SELECT and EXPLAIN are allowed.")]
    pub read_only: Option<bool>,
}

/// Parameter struct for tools that operate on an open bundle by identifier.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BundleKeyParams {
    /// Identifier of the open bundle
    #[schemars(description = "Bundle identifier (as provided when opening/creating)")]
    pub bundle: String,
}

/// MCP server for bundlebase bundles.
///
/// Supports multiple bundles open simultaneously, each identified by a unique bundle name.
#[derive(Clone)]
pub struct BundlebaseMcpServer {
    bundles: Arc<Mutex<HashMap<String, Arc<dyn BundleFacade>>>>,
    tool_router: ToolRouter<Self>,
}

impl BundlebaseMcpServer {
    /// Get an open bundle by identifier, returning an error message if not found.
    async fn get_bundle(&self, bundle: &str) -> Result<Arc<dyn BundleFacade>, String> {
        self.bundles
            .lock()
            .await
            .get(bundle)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "No bundle is open with identifier '{}'. Use list_bundles to see open bundles, \
                     or open_bundle/create_bundle to load one.",
                    bundle
                )
            })
    }
}

#[tool_router]
impl BundlebaseMcpServer {
    pub fn new(bundles: HashMap<String, Arc<dyn BundleFacade>>) -> Self {
        Self {
            bundles: Arc::new(Mutex::new(bundles)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "create_bundle",
        description = "Create a new bundle at the given path with the given bundle identifier. The identifier is used to reference this bundle in all other tools."
    )]
    async fn create_bundle(
        &self,
        Parameters(params): Parameters<BundlePathParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let guard = self.bundles.lock().await;
            if guard.contains_key(&params.bundle) {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "A bundle is already open with identifier '{}'. Use close_bundle first or choose a different identifier.",
                    params.bundle
                ))]));
            }
        }

        match BundleBuilder::create(&params.path, None).await {
            Ok(builder) => {
                let url = builder.url().to_string();
                self.bundles.lock().await.insert(params.bundle.clone(), builder);
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Created bundle '{}' at {}",
                    params.bundle, url
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
        description = "Open an existing bundle at the given path with the given bundle identifier. The identifier is used to reference this bundle in all other tools. Opens in read-write mode by default."
    )]
    async fn open_bundle(
        &self,
        Parameters(params): Parameters<OpenBundleParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let guard = self.bundles.lock().await;
            if guard.contains_key(&params.bundle) {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "A bundle is already open with identifier '{}'. Use close_bundle first or choose a different identifier.",
                    params.bundle
                ))]));
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
                self.bundles.lock().await.insert(params.bundle.clone(), bundle);
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Opened bundle '{}' at {} (version {}, {} commit{})",
                    params.bundle,
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
        description = "Close an open bundle by its identifier."
    )]
    async fn close_bundle(
        &self,
        Parameters(params): Parameters<BundleKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut guard = self.bundles.lock().await;
        if guard.remove(&params.bundle).is_none() {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "No bundle is open with identifier '{}'.",
                params.bundle
            ))]));
        }
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Bundle '{}' closed.",
            params.bundle
        ))]))
    }

    #[tool(
        name = "query",
        description = "Execute a SQL query or bundlebase command against a bundle. Supports SELECT queries, ATTACH/DETACH for data sources, FILTER/SELECT for transformations, COMMIT for saving changes, and all other bundlebase SQL extensions. Results are returned as JSON, limited to 1000 rows."
    )]
    async fn query(
        &self,
        Parameters(params): Parameters<QueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle(&params.bundle).await {
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
        description = "Get a bundle's column schema including column names, data types, and nullability."
    )]
    async fn schema(
        &self,
        Parameters(params): Parameters<BundleKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle(&params.bundle).await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::get_schema(&bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(name = "count", description = "Get the total number of rows in a bundle.")]
    async fn count(
        &self,
        Parameters(params): Parameters<BundleKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle(&params.bundle).await {
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
        description = "Get a sample of rows from a bundle. Returns up to the specified number of rows (default 10, max 1000) as JSON."
    )]
    async fn sample(
        &self,
        Parameters(params): Parameters<SampleParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle(&params.bundle).await {
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
        description = "Get a bundle's status including any uncommitted changes."
    )]
    async fn status(
        &self,
        Parameters(params): Parameters<BundleKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle(&params.bundle).await {
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
        description = "Get a bundle's commit history showing past changes."
    )]
    async fn history(
        &self,
        Parameters(params): Parameters<BundleKeyParams>,
    ) -> Result<CallToolResult, McpError> {
        let bundle = match self.get_bundle(&params.bundle).await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };
        match tools::get_history(&bundle).await {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        name = "list_bundles",
        description = "List all currently open bundles with their identifier, path, name, and description."
    )]
    async fn list_bundles(&self) -> Result<CallToolResult, McpError> {
        let guard = self.bundles.lock().await;
        if guard.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No bundles are currently open.",
            )]));
        }
        let entries: Vec<serde_json::Value> = guard
            .iter()
            .map(|(id, bundle)| {
                serde_json::json!({
                    "bundle": id,
                    "url": bundle.url().to_string(),
                    "name": bundle.name(),
                    "description": bundle.description(),
                })
            })
            .collect();
        match serde_json::to_string_pretty(&entries) {
            Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "JSON error: {}",
                e
            ))])),
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
                 Supports multiple bundles open simultaneously, each identified by a unique bundle name. \
                 Start by calling 'create_bundle' or 'open_bundle' with a bundle name, then use \
                 'query', 'schema', 'sample', 'count' with the same bundle name to interact. \
                 Use 'list_bundles' to see all open bundles, 'close_bundle' to remove one.",
            )
    }
}

/// Start the MCP server over stdio.
///
/// Starts with the given pre-opened bundles (may be empty).
/// Agents can open/close additional bundles via tools.
pub async fn start(
    bundles: HashMap<String, Arc<dyn BundleFacade>>,
) -> Result<(), BundlebaseError> {
    let server = BundlebaseMcpServer::new(bundles);

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
        let mut bundles = HashMap::new();
        bundles.insert("test".to_string(), bundle);
        let server = BundlebaseMcpServer::new(bundles);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "bundlebase");
        assert!(info.instructions.unwrap_or_default().contains("create_bundle"));
    }

    #[tokio::test]
    async fn test_server_instantiation_without_bundle() {
        let server = BundlebaseMcpServer::new(HashMap::new());
        assert!(server.get_bundle("anything").await.is_err());
    }

    #[tokio::test]
    async fn test_get_bundle_returns_error_when_key_missing() {
        let server = BundlebaseMcpServer::new(HashMap::new());
        match server.get_bundle("missing").await {
            Ok(_) => panic!("Expected error when no bundle is open"),
            Err(err) => {
                assert!(err.contains("No bundle is open with identifier 'missing'"));
                assert!(err.contains("create_bundle"));
            }
        }
    }

    #[tokio::test]
    async fn test_get_bundle_returns_bundle_by_key() {
        let bundle = create_test_bundle().await;
        let mut bundles = HashMap::new();
        bundles.insert("mykey".to_string(), bundle);
        let server = BundlebaseMcpServer::new(bundles);
        assert!(server.get_bundle("mykey").await.is_ok());
        assert!(server.get_bundle("otherkey").await.is_err());
    }
}
