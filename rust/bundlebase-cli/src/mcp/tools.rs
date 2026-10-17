//! MCP tool implementations for bundlebase.
//!
//! Each tool handler method uses the existing REPL command infrastructure
//! to parse and execute commands, then formats results as JSON.

use bundlebase::BundleFacade;
use bundlebase_command::parser::quote_identifier;
use bundlebase_command::OutputShape;
use bundlebase_common::progress::ProgressScope;
use serde_json::json;
use std::sync::Arc;

use crate::repl::json_formatter::format_stream_json;

const MCP_QUERY_LIMIT: usize = 1000;

/// Execute a SQL query or bundlebase command against the bundle and return JSON.
pub async fn execute_query(bundle: &Arc<dyn BundleFacade>, sql: &str) -> Result<String, String> {
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
        Ok(Some((stream, shape))) => format_stream_json(stream, Some(shape), Some(MCP_QUERY_LIMIT))
            .await
            .map_err(|e| format!("{}", e)),
        Ok(None) => Ok("OK".to_string()),
        Err(e) => Err(format!("{}", e)),
    }
}

/// Column list (name, type, nullability, description) shared by the
/// `schema` and `describe` tools. Column descriptions (from DESCRIBE COLUMN)
/// give an agent the semantic context that makes its SQL trustworthy.
async fn columns_json(bundle: &Arc<dyn BundleFacade>) -> Result<Vec<serde_json::Value>, String> {
    let schema = bundle.schema().await.map_err(|e| format!("{}", e))?;
    // Resolved once by current column name — no per-column schema lookups.
    let descriptions = bundle.column_descriptions_by_name();
    Ok(schema
        .fields()
        .iter()
        .map(|f| {
            json!({
                "name": f.name(),
                "type": format!("{}", f.data_type()),
                "nullable": f.is_nullable(),
                "description": descriptions.get(f.name()),
            })
        })
        .collect())
}

/// Get the bundle schema as a JSON string.
pub async fn get_schema(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let columns = columns_json(bundle).await?;
    serde_json::to_string_pretty(&columns).map_err(|e| format!("JSON error: {}", e))
}

/// The same validated document returned by Rust and Python semantic_model().
pub async fn get_semantic_model(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let document = bundle.semantic_model().await.map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&document).map_err(|e| e.to_string())
}

/// Apply through the builder's normal granular edit lifecycle.
pub async fn apply_semantic_model(
    bundle: &Arc<dyn BundleFacade>,
    document: &serde_json::Value,
    replace: bool,
) -> Result<String, String> {
    let builder = bundle
        .as_any()
        .downcast_ref::<bundlebase::BundleBuilder>()
        .ok_or("Cannot apply a semantic model to a read-only bundle")?;
    builder
        .apply_semantic_model(document, replace)
        .await
        .map_err(|e| e.to_string())?;
    Ok("Applied semantic model as native metadata edits".to_string())
}

/// Assemble a bundle's full self-description into one agent-facing JSON blob:
/// name, description, lineage, provenance (sources), the transformation record,
/// commit history, and the column schema. An agent that reads this before
/// writing SQL is far less likely to hallucinate — it sees what the data *is*,
/// where it came from, and how it was shaped, not just column names/types.
pub async fn get_describe(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let columns = columns_json(bundle).await?;

    // Native introspection remains available when strict portable projection
    // rejects an unsupported expression, dialect, or physical type.
    let (semantic_model, semantic_model_error) = match bundle.semantic_model().await {
        Ok(document) => (Some(document), None),
        Err(error) => (None, Some(error.to_string())),
    };

    let row_count = bundle.num_rows().await.map_err(|e| format!("{}", e))?;

    // Provenance: where each source's data came from.
    let sources: Vec<serde_json::Value> = bundle
        .sources()
        .values()
        .map(|s| {
            json!({
                "connector": s.connector(),
                "args": s.args(),
                "file_count": s.attached_files().len(),
            })
        })
        .collect();

    // Named metrics — the canonical way to compute the bundle's numbers, so an
    // agent uses the shared definition rather than inventing its own SUM.
    let metrics: Vec<serde_json::Value> = bundle
        .metrics()
        .into_iter()
        .map(|m| {
            json!({
                "name": m.name,
                "expression": m.expression,
                "description": m.description,
            })
        })
        .collect();

    // The transformation record — how the data was shaped, in order.
    let transformations: Vec<String> = bundle
        .operations()
        .iter()
        .map(|op| op.to_string())
        .collect();

    let history = bundle.history();
    let latest_commit = history.last().map(|c| {
        json!({
            "message": c.message,
            "author": c.author,
            "timestamp": c.timestamp,
        })
    });

    let result = json!({
        "id": bundle.id(),
        "name": bundle.name(),
        "description": bundle.description(),
        "version": bundle.version(),
        "url": bundle.url().to_string(),
        "extended_from": bundle.from().map(|u| u.to_string()),
        "row_count": row_count,
        "columns": columns,
        "metrics": metrics,
        "semantic_model": semantic_model,
        "semantic_model_error": semantic_model_error,
        "sources": sources,
        "transformations": transformations,
        "history": {
            "commit_count": history.len(),
            "latest_commit": latest_commit,
        },
    });

    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON error: {}", e))
}

/// One-shot orientation for an agent: the full self-description plus a small
/// data sample, in one call. This is the ideal first thing an agent reads
/// after opening a bundle — it gets what the data is (describe) and what it
/// looks like (sample) together.
pub async fn get_analyze(
    bundle: &Arc<dyn BundleFacade>,
    sample_limit: usize,
) -> Result<String, String> {
    let describe: serde_json::Value = serde_json::from_str(&get_describe(bundle).await?)
        .map_err(|e| format!("JSON error: {}", e))?;
    let sample: serde_json::Value = serde_json::from_str(&get_sample(bundle, sample_limit).await?)
        .map_err(|e| format!("JSON error: {}", e))?;

    let result = json!({
        "describe": describe,
        "sample": sample,
    });
    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON error: {}", e))
}

/// Agent-facing remedy for a text column whose values all parse as `ty`.
/// The command uses the REPL grammar's identifier quoting, which cannot
/// express a name containing `"`; such columns get NORMALIZE COLUMN NAMES
/// as the first step instead.
fn cast_hint(column: &str, ty: &str) -> String {
    let remedy = if column.contains('"') {
        format!(
            "NORMALIZE COLUMN NAMES, then CAST COLUMN <normalized name> TO {ty} \
             (this column's name contains a double quote, which commands cannot reference)"
        )
    } else {
        format!("CAST COLUMN {} TO {ty}", quote_identifier(column))
    };
    format!(
        "Stored as text, but every value parses as {ty}. Comparing text numerically \
         compares lexicographically (\"9\" > \"50\"), which is silently wrong, and \
         aggregates like SUM fail outright. Cast before filtering or aggregating: {remedy}"
    )
}

/// Automated data-quality report (see [`BundleFacade::data_quality`]): per-column
/// null counts/rates and distinct counts, flags an agent can act on (all-null,
/// constant), and cast suggestions for text columns that are really numbers or
/// dates — with a ready-to-run remedy command.
pub async fn get_data_quality(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let report = bundle.data_quality().await.map_err(|e| format!("{}", e))?;
    let columns: Vec<serde_json::Value> = report
        .columns
        .iter()
        .map(|c| {
            let mut v = serde_json::to_value(c).map_err(|e| format!("JSON error: {}", e))?;
            v["cast_hint"] = json!(c.suggested_cast.as_deref().map(|t| cast_hint(&c.name, t)));
            Ok(v)
        })
        .collect::<Result<_, String>>()?;

    serde_json::to_string_pretty(&json!({
        "total_rows": report.total_rows,
        "columns": columns,
    }))
    .map_err(|e| format!("JSON error: {}", e))
}

/// Compute a bundle-defined named metric by name, optionally grouped by one or
/// more dimension columns (see [`BundleFacade::compute_metric`]). This is the
/// *executable* half of the semantic layer: the agent asks for a metric by name
/// and gets the bundle author's canonical computation.
pub async fn get_metric(
    bundle: &Arc<dyn BundleFacade>,
    name: &str,
    dimensions: &[String],
    limit: usize,
) -> Result<String, String> {
    let stream = bundle
        .compute_metric(name, dimensions, Some(limit))
        .await
        .map_err(|e| format!("{}", e))?;
    let rows = format_stream_json(stream, Some(OutputShape::Table), Some(limit))
        .await
        .map_err(|e| format!("{}", e))?;
    let rows_json: serde_json::Value =
        serde_json::from_str(&rows).map_err(|e| format!("JSON error: {}", e))?;

    // Echo the canonical definition so the agent sees what it computed.
    let metric = bundle.metrics().into_iter().find(|m| m.name == name);
    let result = json!({
        "metric": name,
        "expression": metric.as_ref().map(|m| m.expression.clone()),
        "description": metric.and_then(|m| m.description),
        "dimensions": dimensions,
        "rows": rows_json,
    });
    serde_json::to_string_pretty(&result).map_err(|e| format!("JSON error: {}", e))
}

/// Get the row count as a JSON string.
pub async fn get_count(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let count = bundle.num_rows().await.map_err(|e| format!("{}", e))?;
    Ok(count.to_string())
}

/// Get sample rows as a JSON string.
pub async fn get_sample(bundle: &Arc<dyn BundleFacade>, limit: usize) -> Result<String, String> {
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

    let change_list: Vec<serde_json::Value> =
        changes.iter().map(|c| json!(format!("{}", c))).collect();

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
        INIT.call_once(|| {
            bundlebase_catalog::init();
        });
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
        fn start_count(&self) -> u32 {
            self.starts.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn finish_count(&self) -> u32 {
            self.finishes.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn first_operation(&self) -> Option<String> {
            self.first_op.lock().clone()
        }
    }

    impl bundlebase_common::progress::ProgressTracker for CountingTracker {
        fn start(
            &self,
            operation: &str,
            _total: Option<u64>,
        ) -> bundlebase_common::progress::ProgressId {
            self.starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut lock = self.first_op.lock();
            if lock.is_none() {
                *lock = Some(operation.to_string());
            }
            bundlebase_common::progress::ProgressId::new()
        }
        fn update(
            &self,
            _id: bundlebase_common::progress::ProgressId,
            _current: u64,
            _message: Option<&str>,
        ) {
        }
        fn finish(&self, _id: bundlebase_common::progress::ProgressId) {
            self.finishes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        )
        .await;

        assert!(result.is_ok(), "Query failed: {:?}", result);
        assert!(
            tracker.start_count() >= 1,
            "Expected at least one Start event"
        );
        assert!(
            tracker.finish_count() >= 1,
            "Expected at least one Finish event"
        );

        let op = tracker
            .first_operation()
            .expect("No operation name recorded");
        assert!(
            op.contains("SELECT 1"),
            "Operation should contain SQL, got: {}",
            op
        );
    }

    /// Verify progress events fire for a bundlebase command (SHOW STATUS), not just SELECT.
    #[tokio::test]
    async fn test_command_emits_progress_events() {
        let bundle = create_test_bundle().await;
        let tracker = CountingTracker::new();

        let result = run_with_tracker(
            Arc::new(tracker.clone()),
            execute_query(&bundle, "SHOW STATUS"),
        )
        .await;

        assert!(result.is_ok(), "SHOW STATUS failed: {:?}", result);
        assert!(tracker.start_count() >= 1, "No Start event for SHOW STATUS");
        assert!(
            tracker.finish_count() >= 1,
            "No Finish event for SHOW STATUS"
        );
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
    async fn test_get_describe_assembles_self_description() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        // Downcast to the builder so we can set name/description + attach data.
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("test bundle is a builder");
        builder.set_name("Sales").await.expect("set name");
        builder
            .set_description("Monthly sales extract")
            .await
            .expect("set description");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");

        let json: serde_json::Value = serde_json::from_str(
            &get_describe(&bundle)
                .await
                .expect("describe should succeed"),
        )
        .expect("valid JSON");

        assert_eq!(json["name"], "Sales");
        assert_eq!(json["description"], "Monthly sales extract");
        assert!(json["version"].is_string());
        assert!(
            json["row_count"].as_u64().expect("row_count") > 0,
            "attached data should be counted"
        );
        assert!(
            json["columns"].as_array().expect("columns").len() > 1,
            "schema columns must be listed"
        );
        // The ATTACH is part of the transformation record.
        let transforms = json["transformations"].as_array().expect("transformations");
        assert!(
            transforms
                .iter()
                .any(|t| t.as_str().unwrap_or("").contains("ATTACH")),
            "transformation record must include the ATTACH, got: {transforms:?}"
        );
        assert!(json["history"]["commit_count"].is_number());
    }

    #[tokio::test]
    async fn test_column_descriptions_surface_in_schema_and_describe() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");

        // Set a column description via the SQL command through the MCP query path.
        execute_query(
            &bundle,
            "DESCRIBE COLUMN salary AS 'annual gross pay in USD'",
        )
        .await
        .expect("describe column");

        // schema tool surfaces it.
        let schema: Vec<serde_json::Value> =
            serde_json::from_str(&get_schema(&bundle).await.expect("schema")).expect("json");
        let salary = schema
            .iter()
            .find(|c| c["name"] == "salary")
            .expect("salary column");
        assert_eq!(salary["description"], "annual gross pay in USD");

        // describe tool surfaces it too.
        let desc: serde_json::Value =
            serde_json::from_str(&get_describe(&bundle).await.expect("describe")).expect("json");
        let salary_d = desc["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .find(|c| c["name"] == "salary")
            .expect("salary");
        assert_eq!(salary_d["description"], "annual gross pay in USD");

        // A column with no description reports null.
        let other = schema
            .iter()
            .find(|c| c["name"] != "salary")
            .expect("another column");
        assert!(other["description"].is_null());
    }

    #[tokio::test]
    async fn test_metrics_surface_in_describe() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");

        execute_query(
            &bundle,
            "CREATE METRIC avg_salary AS 'AVG(salary)' DESCRIPTION 'mean salary in USD'",
        )
        .await
        .expect("create metric");

        let desc: serde_json::Value =
            serde_json::from_str(&get_describe(&bundle).await.expect("describe")).expect("json");
        let metrics = desc["metrics"].as_array().expect("metrics array");
        let m = metrics
            .iter()
            .find(|m| m["name"] == "avg_salary")
            .expect("avg_salary metric");
        assert_eq!(m["expression"], "AVG(salary)");
        assert_eq!(m["description"], "mean salary in USD");
    }

    #[tokio::test]
    async fn test_get_describe_empty_bundle() {
        let bundle = create_test_bundle().await;
        let json: serde_json::Value =
            serde_json::from_str(&get_describe(&bundle).await.expect("describe")).expect("json");
        // Empty bundle: no name/description, no sources, zero rows — but valid.
        assert_eq!(json["row_count"], 0);
        assert!(json["sources"].as_array().expect("sources").is_empty());
        assert!(json["name"].is_null());
    }

    #[tokio::test]
    async fn test_semantic_tools_share_projection_and_native_edits() {
        let bundle = create_test_bundle().await;
        let mut document = bundle.semantic_model().await.expect("projection");
        let projected: serde_json::Value =
            serde_json::from_str(&get_semantic_model(&bundle).await.expect("semantic tool"))
                .expect("JSON");
        assert_eq!(projected, document);

        document["ai_context"] = json!({"instructions": "Use declared metrics"});
        apply_semantic_model(&bundle, &document, false)
            .await
            .expect("native apply");
        let description: serde_json::Value =
            serde_json::from_str(&get_describe(&bundle).await.expect("describe")).expect("JSON");
        assert_eq!(
            description["semantic_model"],
            bundle.semantic_model().await.expect("current projection")
        );
        assert!(description["semantic_model_error"].is_null());

        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder.commit("semantic model").await.expect("commit");
        let readonly: Arc<dyn BundleFacade> = bundlebase::Bundle::open(bundle.url().as_str(), None)
            .await
            .expect("open");
        assert!(apply_semantic_model(&readonly, &document, false)
            .await
            .expect_err("read-only write must fail")
            .contains("read-only"));
    }

    #[tokio::test]
    async fn test_analyze_combines_describe_and_sample() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");

        let json: serde_json::Value =
            serde_json::from_str(&get_analyze(&bundle, 3).await.expect("analyze")).expect("json");
        // Has both the describe blob and a bounded sample.
        assert!(
            json["describe"]["columns"]
                .as_array()
                .expect("columns")
                .len()
                > 1
        );
        let sample = json["sample"].as_array().expect("sample");
        assert!(sample.len() <= 3 && !sample.is_empty());
    }

    #[tokio::test]
    async fn test_data_quality_reports_nulls_and_distincts() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");

        let json: serde_json::Value =
            serde_json::from_str(&get_data_quality(&bundle).await.expect("data_quality"))
                .expect("json");

        let total = json["total_rows"].as_i64().expect("total");
        assert!(total > 0);
        let cols = json["columns"].as_array().expect("columns");
        assert!(!cols.is_empty());

        // id is unique and non-null: distinct == total, null_count == 0.
        let id = cols.iter().find(|c| c["name"] == "id").expect("id column");
        assert_eq!(id["null_count"], 0);
        assert_eq!(id["distinct"].as_i64().expect("distinct"), total);
        assert_eq!(id["constant"], false);
        assert_eq!(id["all_null"], false);
    }

    #[tokio::test]
    async fn test_data_quality_empty_bundle() {
        let bundle = create_test_bundle().await;
        let json: serde_json::Value =
            serde_json::from_str(&get_data_quality(&bundle).await.expect("dq")).expect("json");
        assert_eq!(json["total_rows"], 0);
        assert!(json["columns"].as_array().expect("columns").is_empty());
    }

    /// Every CSV column imports as text. data_quality must tell an agent which
    /// of those text columns are really numbers/dates, so it casts instead of
    /// silently comparing lexicographically.
    #[tokio::test]
    async fn test_data_quality_suggests_casts_for_text_columns() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("customers-0-100.csv"),
                None,
            )
            .await
            .expect("attach");

        let json: serde_json::Value =
            serde_json::from_str(&get_data_quality(&bundle).await.expect("dq")).expect("json");
        let cols = json["columns"].as_array().expect("columns");
        let col = |n: &str| {
            cols.iter()
                .find(|c| c["name"] == n)
                .expect("column present")
        };

        // Numeric-looking text -> suggest an integer cast, with an actionable hint.
        assert_eq!(col("Index")["suggested_cast"], "BIGINT");
        assert!(
            col("Index")["cast_hint"]
                .as_str()
                .expect("hint")
                .contains("CAST COLUMN"),
            "hint must give the remedy"
        );

        // ISO dates -> suggest DATE.
        assert_eq!(col("Subscription Date")["suggested_cast"], "DATE");

        // Genuinely-textual columns get no suggestion.
        assert!(col("Customer Id")["suggested_cast"].is_null());
        assert!(col("First Name")["suggested_cast"].is_null());
    }

    /// Zero-padded values ("007") parse as integers but must NOT be suggested for
    /// a numeric cast -- casting would silently destroy the padding.
    #[tokio::test]
    async fn test_data_quality_does_not_suggest_cast_for_zero_padded() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("zero-padded.csv"),
                None,
            )
            .await
            .expect("attach");

        let json: serde_json::Value =
            serde_json::from_str(&get_data_quality(&bundle).await.expect("dq")).expect("json");
        let cols = json["columns"].as_array().expect("columns");
        let col = |n: &str| {
            cols.iter()
                .find(|c| c["name"] == n)
                .expect("column present")
        };

        // "007"/"012" would cast cleanly to BIGINT -- but the padding is meaningful.
        assert!(
            col("code")["suggested_cast"].is_null(),
            "zero-padded identifiers must not be suggested for a numeric cast"
        );
        // A genuine decimal column is still suggested.
        assert_eq!(col("amount")["suggested_cast"], "DOUBLE");
    }

    /// The cast hint's remedy must be a command the REPL grammar can parse,
    /// including for names that need quoting.
    #[test]
    fn test_cast_hint_emits_parseable_command() {
        for name in ["Index", "Subscription Date", "weird-name"] {
            let hint = cast_hint(name, "BIGINT");
            let cmd = &hint[hint.find("CAST COLUMN").expect("has remedy")..];
            match bundlebase_command::parser::parse_command(cmd)
                .unwrap_or_else(|e| panic!("hint for {name:?} must parse: {cmd}: {e}"))
            {
                bundlebase_command::BundleCommand::CastColumn(c) => {
                    assert_eq!(c.name, name)
                }
                _ => panic!("expected CAST COLUMN for {name:?}"),
            }
        }
        // A name containing '"' cannot be referenced by any command, so the
        // hint must not pretend otherwise.
        let hint = cast_hint("say \"hi\"", "BIGINT");
        assert!(hint.contains("NORMALIZE COLUMN NAMES"), "{hint}");
        assert!(!hint.contains("\"say"), "{hint}");
    }

    /// A real column named `no_data` is profiled, not mistaken for the
    /// empty-bundle sentinel.
    #[tokio::test]
    async fn test_data_quality_reports_real_no_data_column() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");
        builder
            .rename_column("gender", "no_data")
            .await
            .expect("rename");

        let json: serde_json::Value =
            serde_json::from_str(&get_data_quality(&bundle).await.expect("dq")).expect("json");
        let cols = json["columns"].as_array().expect("columns");
        assert!(
            cols.iter().any(|c| c["name"] == "no_data"),
            "real no_data column must be reported: {cols:?}"
        );
    }

    /// After RENAME COLUMN the metric tool still evaluates and echoes the
    /// expression with the current column name.
    #[tokio::test]
    async fn test_metric_after_rename_column() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");
        execute_query(&bundle, "CREATE METRIC payroll AS 'SUM(salary)'")
            .await
            .expect("create metric");
        execute_query(&bundle, "RENAME COLUMN salary TO pay")
            .await
            .expect("rename");

        let json: serde_json::Value = serde_json::from_str(
            &get_metric(&bundle, "payroll", &[], 10)
                .await
                .expect("metric"),
        )
        .expect("json");
        assert_eq!(json["expression"], "SUM(pay)");
        assert!(json["rows"][0]["payroll"].is_number(), "{}", json["rows"]);

        let desc: serde_json::Value =
            serde_json::from_str(&get_describe(&bundle).await.expect("describe")).expect("json");
        assert_eq!(desc["metrics"][0]["expression"], "SUM(pay)");
    }

    #[tokio::test]
    async fn test_metric_computes_ungrouped() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");
        execute_query(
            &bundle,
            "CREATE METRIC avg_salary AS 'AVG(salary)' DESCRIPTION 'mean salary in USD'",
        )
        .await
        .expect("create metric");

        let json: serde_json::Value = serde_json::from_str(
            &get_metric(&bundle, "avg_salary", &[], 10)
                .await
                .expect("metric"),
        )
        .expect("json");

        // Echoes the canonical definition so the agent sees what it computed.
        assert_eq!(json["metric"], "avg_salary");
        assert_eq!(json["expression"], "AVG(salary)");
        assert_eq!(json["description"], "mean salary in USD");
        // An ungrouped aggregate yields exactly one row carrying the metric value.
        let rows = json["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "ungrouped metric returns a single row");
        assert!(
            rows[0]["avg_salary"].is_number(),
            "metric column must hold the computed number, got: {}",
            rows[0]
        );
    }

    #[tokio::test]
    async fn test_metric_grouped_by_dimension() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");
        execute_query(&bundle, "CREATE METRIC headcount AS 'COUNT(*)'")
            .await
            .expect("create metric");

        let json: serde_json::Value = serde_json::from_str(
            &get_metric(&bundle, "headcount", &["gender".to_string()], 50)
                .await
                .expect("metric"),
        )
        .expect("json");

        let rows = json["rows"].as_array().expect("rows");
        assert!(rows.len() >= 2, "grouping by gender yields multiple groups");
        // Each row carries the dimension value plus the metric.
        for row in rows {
            assert!(
                row.get("gender").is_some(),
                "dimension column present: {row}"
            );
            assert!(row["headcount"].is_number(), "metric present: {row}");
        }
        assert_eq!(json["dimensions"][0], "gender");
    }

    #[tokio::test]
    async fn test_metric_unknown_name_lists_available() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");
        execute_query(&bundle, "CREATE METRIC avg_salary AS 'AVG(salary)'")
            .await
            .expect("create metric");

        let err = get_metric(&bundle, "nonexistent", &[], 10)
            .await
            .expect_err("unknown metric must error");
        // The error names the missing metric and lists what is available so the
        // agent can self-correct without another round-trip.
        assert!(err.contains("nonexistent"), "names the miss: {err}");
        assert!(err.contains("avg_salary"), "lists available metrics: {err}");
    }

    #[tokio::test]
    async fn test_metric_unknown_dimension_rejected() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle
            .as_any()
            .downcast_ref::<BundleBuilder>()
            .expect("builder");
        builder
            .attach(
                bundlebase::test_utils::test_datafile("userdata.parquet"),
                None,
            )
            .await
            .expect("attach");
        execute_query(&bundle, "CREATE METRIC headcount AS 'COUNT(*)'")
            .await
            .expect("create metric");

        let err = get_metric(&bundle, "headcount", &["not_a_column".to_string()], 10)
            .await
            .expect_err("unknown dimension must error");
        assert!(
            err.contains("not_a_column"),
            "names the bad dimension: {err}"
        );
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
