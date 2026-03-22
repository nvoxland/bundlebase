//! MCP tool implementations for bundlebase.
//!
//! Each tool handler method uses the existing REPL command infrastructure
//! to parse and execute commands, then formats results as JSON.

use bundlebase::bundle::OutputShape;
use bundlebase::BundleFacade;
use serde_json::json;
use std::sync::Arc;

use crate::repl::json_formatter::format_stream_json;

const MCP_QUERY_LIMIT: usize = 1000;

/// Execute a SQL query or bundlebase command against the bundle and return JSON.
pub async fn execute_query(
    bundle: &Arc<dyn BundleFacade>,
    sql: &str,
) -> Result<String, String> {
    use crate::repl::commands;

    let cmds = commands::parse(sql).map_err(|e| e.to_string())?;

    let mut last_output = "OK".to_string();
    for cmd in cmds {
        match commands::execute(cmd, bundle).await {
            Ok(Some((stream, shape))) => {
                last_output = format_stream_json(stream, Some(shape), Some(MCP_QUERY_LIMIT))
                    .await
                    .map_err(|e| format!("{}", e))?;
            }
            Ok(None) => {
                last_output = "OK".to_string();
            }
            Err(e) => return Err(format!("{}", e)),
        }
    }
    Ok(last_output)
}

/// Get the bundle schema as a JSON string.
pub async fn get_schema(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let schema = bundle.schema().await.map_err(|e| format!("{}", e))?;

    let columns: Vec<serde_json::Value> = schema
        .fields()
        .iter()
        .map(|f| {
            json!({
                "name": f.name(),
                "type": format!("{}", f.data_type()),
                "nullable": f.is_nullable(),
            })
        })
        .collect();

    serde_json::to_string_pretty(&columns).map_err(|e| format!("JSON error: {}", e))
}

/// Get the row count as a JSON string.
pub async fn get_count(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let count = bundle.num_rows().await.map_err(|e| format!("{}", e))?;
    Ok(count.to_string())
}

/// Get sample rows as a JSON string.
pub async fn get_sample(
    bundle: &Arc<dyn BundleFacade>,
    limit: usize,
) -> Result<String, String> {
    let sql = format!("SELECT * FROM bundle LIMIT {}", limit);
    let stream = bundle
        .query(&sql, vec![], Some(limit))
        .await
        .map_err(|e| format!("{}", e))?;

    format_stream_json(stream, Some(OutputShape::Table), Some(limit))
        .await
        .map_err(|e| format!("{}", e))
}

/// Get the bundle status as a JSON string.
pub async fn get_status(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let status = bundle.status();
    let changes = bundle.status_changes();

    let change_list: Vec<serde_json::Value> = changes
        .iter()
        .map(|c| json!(format!("{}", c)))
        .collect();

    let result = json!({
        "status": format!("{}", status),
        "changes": change_list,
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON error: {}", e))
}

/// Get the bundle commit history as a JSON string.
pub async fn get_history(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let history = bundle.history();

    let commits: Vec<serde_json::Value> = history
        .iter()
        .map(|c| {
            json!({
                "author": c.author,
                "message": c.message,
                "timestamp": c.timestamp,
            })
        })
        .collect();

    serde_json::to_string_pretty(&commits).map_err(|e| format!("JSON error: {}", e))
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
        BundleBuilder::create(&format!("memory:///mcp_tools_test_{}", ts), None)
            .await
            .expect("Failed to create test bundle")
    }

    #[tokio::test]
    async fn test_get_schema_empty_bundle() {
        let bundle = create_test_bundle().await;
        let result = get_schema(&bundle).await;
        assert!(result.is_ok());
        let json: Vec<serde_json::Value> =
            serde_json::from_str(&result.expect("Should succeed")).expect("Should be valid JSON");
        // Empty bundle has a sentinel no_data field
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["name"], "no_data");
    }

    #[tokio::test]
    async fn test_get_count_empty_bundle() {
        let bundle = create_test_bundle().await;
        let result = get_count(&bundle).await;
        assert!(result.is_ok());
        assert_eq!(result.expect("Should succeed"), "0");
    }

    #[tokio::test]
    async fn test_get_sample_empty_bundle() {
        let bundle = create_test_bundle().await;
        let result = get_sample(&bundle, 10).await;
        assert!(result.is_ok());
        let json: Vec<serde_json::Value> =
            serde_json::from_str(&result.expect("Should succeed")).expect("Should be valid JSON");
        assert_eq!(json.len(), 0);
    }

    #[tokio::test]
    async fn test_get_status() {
        let bundle = create_test_bundle().await;
        let result = get_status(&bundle).await;
        assert!(result.is_ok());
        let json: serde_json::Value =
            serde_json::from_str(&result.expect("Should succeed")).expect("Should be valid JSON");
        assert!(json["status"].is_string());
        assert!(json["changes"].is_array());
    }

    #[tokio::test]
    async fn test_get_history() {
        let bundle = create_test_bundle().await;
        let result = get_history(&bundle).await;
        assert!(result.is_ok());
        let json: Vec<serde_json::Value> =
            serde_json::from_str(&result.expect("Should succeed")).expect("Should be valid JSON");
        // New bundle with no commits
        assert!(json.is_empty());
    }

    #[tokio::test]
    async fn test_execute_query_invalid_sql() {
        let bundle = create_test_bundle().await;
        let result = execute_query(&bundle, "NOT VALID SQL AT ALL").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_query_select() {
        let bundle = create_test_bundle().await;
        let result = execute_query(&bundle, "SELECT 1 as value").await;
        assert!(result.is_ok(), "Query failed: {:?}", result);
    }
}
