//! MCP tool implementations for bundlebase.
//!
//! Each tool handler method uses the existing REPL command infrastructure
//! to parse and execute commands, then formats results as JSON.

use bundlebase_command::OutputShape;
use bundlebase::BundleFacade;
use bundlebase_common::progress::ProgressScope;
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

    // Top-level progress scope: emits start/finish for every command, even those
    // that have no internal progress instrumentation (e.g. CREATE SOURCE, long SELECTs).
    // Truncate at 80 chars so long queries don't produce unwieldy notification messages.
    let label = sql.get(..80).unwrap_or(sql);
    let _progress = ProgressScope::new(label, None);

    let mut cmds = commands::parse(sql).map_err(|e| e.to_string())?;

    if cmds.len() > 1 {
        return Err(
            "MCP supports only one statement at a time. Send each statement as a separate tool call instead of separating with ';'.".to_string()
        );
    }

    let cmd = cmds.pop().ok_or_else(|| "Empty command".to_string())?;
    match commands::execute(cmd, bundle).await {
        Ok(Some((stream, shape))) => {
            format_stream_json(stream, Some(shape), Some(MCP_QUERY_LIMIT))
                .await
                .map_err(|e| format!("{}", e))
        }
        Ok(None) => Ok("OK".to_string()),
        Err(e) => Err(format!("{}", e)),
    }
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
    use bundlebase_common::progress::run_with_tracker;
    use std::sync::Arc;

    fn init() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| { bundlebase_catalog::init(); });
    }

    async fn create_test_bundle() -> Arc<dyn BundleFacade> {
        init();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("System time before UNIX epoch")
            .as_nanos();
        BundleBuilder::create(&format!("memory:///mcp_tools_test_{}", ts), None)
            .await
            .expect("Failed to create test bundle")
    }

    /// A simple tracker that counts start/finish calls and captures the first operation name.
    #[derive(Clone)]
    struct CountingTracker {
        starts: Arc<std::sync::atomic::AtomicU32>,
        finishes: Arc<std::sync::atomic::AtomicU32>,
        first_op: Arc<parking_lot::Mutex<Option<String>>>,
    }

    impl CountingTracker {
        fn new() -> Self {
            Self {
                starts: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                finishes: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                first_op: Arc::new(parking_lot::Mutex::new(None)),
            }
        }
        fn start_count(&self) -> u32 { self.starts.load(std::sync::atomic::Ordering::SeqCst) }
        fn finish_count(&self) -> u32 { self.finishes.load(std::sync::atomic::Ordering::SeqCst) }
        fn first_operation(&self) -> Option<String> { self.first_op.lock().clone() }
    }

    impl bundlebase_common::progress::ProgressTracker for CountingTracker {
        fn start(&self, operation: &str, _total: Option<u64>) -> bundlebase_common::progress::ProgressId {
            self.starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut lock = self.first_op.lock();
            if lock.is_none() {
                *lock = Some(operation.to_string());
            }
            bundlebase_common::progress::ProgressId::new()
        }
        fn update(&self, _id: bundlebase_common::progress::ProgressId, _current: u64, _message: Option<&str>) {}
        fn finish(&self, _id: bundlebase_common::progress::ProgressId) {
            self.finishes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Verify that execute_query emits a Start and Finish progress event for every command,
    /// even commands (like CREATE SOURCE) that have no internal progress instrumentation.
    #[tokio::test]
    async fn test_execute_query_emits_progress_events() {
        let bundle = create_test_bundle().await;
        let tracker = CountingTracker::new();

        let result = run_with_tracker(
            Arc::new(tracker.clone()),
            execute_query(&bundle, "SELECT 1 AS value"),
        ).await;

        assert!(result.is_ok(), "Query failed: {:?}", result);
        assert!(tracker.start_count() >= 1, "Expected at least one Start event");
        assert!(tracker.finish_count() >= 1, "Expected at least one Finish event");

        let op = tracker.first_operation().expect("No operation name recorded");
        assert!(op.contains("SELECT 1"), "Operation should contain SQL, got: {}", op);
    }

    /// Verify progress events fire for a bundlebase command (SHOW STATUS), not just SELECT.
    #[tokio::test]
    async fn test_command_emits_progress_events() {
        let bundle = create_test_bundle().await;
        let tracker = CountingTracker::new();

        let result = run_with_tracker(
            Arc::new(tracker.clone()),
            execute_query(&bundle, "SHOW STATUS"),
        ).await;

        assert!(result.is_ok(), "SHOW STATUS failed: {:?}", result);
        assert!(tracker.start_count() >= 1, "No Start event for SHOW STATUS");
        assert!(tracker.finish_count() >= 1, "No Finish event for SHOW STATUS");
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
