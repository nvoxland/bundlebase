//! MCP tool implementations for bundlebase.
//!
//! Each tool handler method uses the existing REPL command infrastructure
//! to parse and execute commands, then formats results as JSON.

use bundlebase::BundleFacade;
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

/// Get the bundle schema as a JSON string.
pub async fn get_schema(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let schema = bundle.schema().await.map_err(|e| format!("{}", e))?;

    let descriptions = bundle.column_descriptions();
    let columns: Vec<serde_json::Value> = schema
        .fields()
        .iter()
        .map(|f| {
            // Column descriptions (from DESCRIBE COLUMN) give an agent the
            // semantic context that makes its SQL trustworthy.
            let description = bundle
                .column_id(f.name())
                .and_then(|id| descriptions.get(&id).cloned());
            json!({
                "name": f.name(),
                "type": format!("{}", f.data_type()),
                "nullable": f.is_nullable(),
                "description": description,
            })
        })
        .collect();

    serde_json::to_string_pretty(&columns).map_err(|e| format!("JSON error: {}", e))
}

/// Assemble a bundle's full self-description into one agent-facing JSON blob:
/// name, description, lineage, provenance (sources), the transformation record,
/// commit history, and the column schema. An agent that reads this before
/// writing SQL is far less likely to hallucinate — it sees what the data *is*,
/// where it came from, and how it was shaped, not just column names/types.
pub async fn get_describe(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    let schema = bundle.schema().await.map_err(|e| format!("{}", e))?;
    let descriptions = bundle.column_descriptions();
    let columns: Vec<serde_json::Value> = schema
        .fields()
        .iter()
        .map(|f| {
            // Column descriptions (from DESCRIBE COLUMN) give an agent the
            // semantic context that makes its SQL trustworthy.
            let description = bundle
                .column_id(f.name())
                .and_then(|id| descriptions.get(&id).cloned());
            json!({
                "name": f.name(),
                "type": format!("{}", f.data_type()),
                "nullable": f.is_nullable(),
                "description": description,
            })
        })
        .collect();

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
    let transformations: Vec<String> =
        bundle.operations().iter().map(|op| op.to_string()).collect();

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
pub async fn get_analyze(bundle: &Arc<dyn BundleFacade>, sample_limit: usize) -> Result<String, String> {
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

/// Where each column's aggregates land in the single-scan result row.
struct ColSlots {
    name: String,
    is_text: bool,
    non_null: usize,
    distinct: usize,
    /// Text-only probes: how many non-null values parse cleanly as each type,
    /// plus a count of zero-padded values (e.g. "007") that must NOT be cast.
    as_int: Option<usize>,
    as_float: Option<usize>,
    as_date: Option<usize>,
    zero_padded: Option<usize>,
}

/// Automated data-quality report: per-column null counts/rates and distinct
/// counts, with flags an agent can act on (all-null, constant).
///
/// For text columns this also answers the question the CSV/JSONL all-Utf8
/// import raises: *is this text actually a number or a date?* Every CSV/TSV/
/// JSONL column arrives as Utf8, and a numeric comparison on text compares
/// lexicographically ("9" > "50") — wrong, and silently so, with no error to
/// catch it. We probe each text column with TRY_CAST over the same scan (so it
/// costs nothing extra) and, when *every* non-null value parses as one type,
/// report a `suggested_cast` so an agent casts before it filters or aggregates.
///
/// Zero-padded values ("007", zip codes) are detected and suppress a numeric
/// suggestion: casting them to a number silently destroys the padding.
pub async fn get_data_quality(bundle: &Arc<dyn BundleFacade>) -> Result<String, String> {
    use futures::StreamExt;

    let schema = bundle.schema().await.map_err(|e| format!("{}", e))?;
    // Real columns (name, is_text), skipping the empty-bundle sentinel.
    let cols: Vec<(String, bool)> = schema
        .fields()
        .iter()
        .filter(|f| f.name() != "no_data")
        .map(|f| {
            let is_text = matches!(
                f.data_type(),
                arrow::datatypes::DataType::Utf8
                    | arrow::datatypes::DataType::LargeUtf8
                    | arrow::datatypes::DataType::Utf8View
            );
            (f.name().to_string(), is_text)
        })
        .collect();

    if cols.is_empty() {
        return serde_json::to_string_pretty(&json!({
            "total_rows": 0,
            "columns": [],
        }))
        .map_err(|e| format!("JSON error: {}", e));
    }

    // Quote a column name for SQL (double-quote, escape internal quotes).
    let q = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));

    // DataFusion's expression naming is transparent to casts: COUNT(TRY_CAST(x AS t))
    // and COUNT(x) both auto-name to `count(x)`, so several probes over one column
    // collide as "duplicate unqualified field name". Project the casts into a
    // subquery as distinctly *aliased* columns and aggregate those instead.
    let mut inner: Vec<String> = Vec::new();
    let mut selects = vec!["COUNT(*) AS c_total".to_string()];
    let mut slot = 1usize;
    let mut next = |selects: &mut Vec<String>, expr: String| -> usize {
        selects.push(expr);
        let s = slot;
        slot += 1;
        s
    };

    let mut layout: Vec<ColSlots> = Vec::with_capacity(cols.len());
    for (i, (name, is_text)) in cols.iter().enumerate() {
        inner.push(format!("{} AS c{}", q(name), i));
        let non_null = next(&mut selects, format!("COUNT(c{}) AS c{}_nn", i, i));
        let distinct = next(&mut selects, format!("COUNT(DISTINCT c{}) AS c{}_dc", i, i));
        let (mut as_int, mut as_float, mut as_date, mut zero_padded) = (None, None, None, None);
        if *is_text {
            inner.push(format!("TRY_CAST({} AS BIGINT) AS c{}_i", q(name), i));
            inner.push(format!("TRY_CAST({} AS DOUBLE) AS c{}_f", q(name), i));
            inner.push(format!("TRY_CAST({} AS DATE) AS c{}_d", q(name), i));
            // '0' followed by a digit — "007", "01234". Not "0.5", a real number.
            inner.push(format!(
                "CASE WHEN regexp_like({}, '^0[0-9]') THEN 1 END AS c{}_z",
                q(name),
                i
            ));
            as_int = Some(next(&mut selects, format!("COUNT(c{}_i) AS c{}_ci", i, i)));
            as_float = Some(next(&mut selects, format!("COUNT(c{}_f) AS c{}_cf", i, i)));
            as_date = Some(next(&mut selects, format!("COUNT(c{}_d) AS c{}_cd", i, i)));
            zero_padded = Some(next(&mut selects, format!("COUNT(c{}_z) AS c{}_cz", i, i)));
        }
        layout.push(ColSlots {
            name: name.clone(),
            is_text: *is_text,
            non_null,
            distinct,
            as_int,
            as_float,
            as_date,
            zero_padded,
        });
    }
    let sql = format!(
        "SELECT {} FROM (SELECT {} FROM bundle)",
        selects.join(", "),
        inner.join(", ")
    );

    let mut stream = bundle
        .query(&sql, vec![], None)
        .await
        .map_err(|e| format!("{}", e))?;
    let mut batch = None;
    while let Some(b) = stream.next().await {
        let b = b.map_err(|e| format!("{}", e))?;
        if b.num_rows() > 0 {
            batch = Some(b);
            break;
        }
    }
    let batch = batch.ok_or_else(|| "data-quality query returned no rows".to_string())?;

    let read_i64 = |col: usize| -> i64 {
        batch
            .column(col)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .map(|a| a.value(0))
            .unwrap_or(0)
    };
    let total = read_i64(0);

    let columns: Vec<serde_json::Value> = layout
        .iter()
        .map(|c| {
            let non_null = read_i64(c.non_null);
            let distinct = read_i64(c.distinct);
            let null_count = total - non_null;
            let null_rate = if total > 0 {
                Some((null_count as f64) / (total as f64))
            } else {
                None
            };

            // Suggest a cast only when EVERY non-null value parses as that type.
            let suggested_cast: Option<&str> = if c.is_text && non_null > 0 {
                let all = |slot: Option<usize>| slot.map(read_i64).unwrap_or(0) == non_null;
                let padded = c.zero_padded.map(read_i64).unwrap_or(0) > 0;
                if !padded && all(c.as_int) {
                    Some("BIGINT")
                } else if !padded && all(c.as_float) {
                    Some("DOUBLE")
                } else if all(c.as_date) {
                    Some("DATE")
                } else {
                    None
                }
            } else {
                None
            };

            json!({
                "name": c.name,
                "non_null": non_null,
                "null_count": null_count,
                "null_rate": null_rate,
                "distinct": distinct,
                "all_null": non_null == 0 && total > 0,
                "constant": non_null > 0 && distinct == 1,
                "suggested_cast": suggested_cast,
                "cast_hint": suggested_cast.map(|t| format!(
                    "Stored as text, but every value parses as {t}. Comparing text \
                     numerically compares lexicographically (\"9\" > \"50\"), which is \
                     silently wrong, and aggregates like SUM fail outright. Cast before \
                     filtering or aggregating: CAST COLUMN {} TO {t}",
                    q(&c.name)
                )),
            })
        })
        .collect();

    serde_json::to_string_pretty(&json!({
        "total_rows": total,
        "columns": columns,
    }))
    .map_err(|e| format!("JSON error: {}", e))
}

/// Compute a bundle-defined named metric by name, optionally grouped by one or
/// more dimension columns. This is the *executable* half of the semantic layer:
/// the agent asks for a metric by name and gets the bundle author's canonical
/// computation, rather than hand-writing an aggregate that may quietly diverge
/// from the intended definition.
pub async fn get_metric(
    bundle: &Arc<dyn BundleFacade>,
    name: &str,
    dimensions: &[String],
    limit: usize,
) -> Result<String, String> {
    // Resolve the metric definition (metrics() already applies last-write-wins).
    let metrics = bundle.metrics();
    let metric = metrics.iter().find(|m| m.name == name).ok_or_else(|| {
        let available: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
        if available.is_empty() {
            format!(
                "No metric named '{}'. This bundle defines no metrics; create one with \
                 CREATE METRIC <name> AS '<expression>'.",
                name
            )
        } else {
            format!(
                "No metric named '{}'. Available metrics: {}.",
                name,
                available.join(", ")
            )
        }
    })?;

    // Validate dimensions against the real schema: an agent may only group by
    // columns that exist. This guards the generated SQL and yields a clear,
    // self-correcting error instead of an opaque query failure.
    let schema = bundle.schema().await.map_err(|e| format!("{}", e))?;
    let valid_cols: std::collections::HashSet<&str> =
        schema.fields().iter().map(|f| f.name().as_str()).collect();
    for d in dimensions {
        if !valid_cols.contains(d.as_str()) {
            let mut cols: Vec<&str> = valid_cols.iter().copied().collect();
            cols.sort_unstable();
            return Err(format!(
                "Unknown dimension column '{}'. Available columns: {}.",
                d,
                cols.join(", ")
            ));
        }
    }

    // Quote an identifier for SQL (double-quote, escape internal quotes).
    let q = |ident: &str| format!("\"{}\"", ident.replace('"', "\"\""));

    let mut select_parts: Vec<String> = dimensions.iter().map(|d| q(d)).collect();
    // The metric expression is bundle-author-defined (trusted, same provenance
    // as any recorded operation); parenthesize and alias it with the metric name.
    select_parts.push(format!("({}) AS {}", metric.expression, q(name)));

    let mut sql = format!("SELECT {} FROM bundle", select_parts.join(", "));
    if !dimensions.is_empty() {
        let dims_sql = dimensions.iter().map(|d| q(d)).collect::<Vec<_>>().join(", ");
        // GROUP BY the dimensions; ORDER BY them for stable, readable output.
        sql.push_str(&format!(" GROUP BY {} ORDER BY {}", dims_sql, dims_sql));
    }
    sql.push_str(&format!(" LIMIT {}", limit));

    let stream = bundle
        .query(&sql, vec![], Some(limit))
        .await
        .map_err(|e| format!("{}", e))?;
    let rows = format_stream_json(stream, Some(OutputShape::Table), Some(limit))
        .await
        .map_err(|e| format!("{}", e))?;
    let rows_json: serde_json::Value =
        serde_json::from_str(&rows).map_err(|e| format!("JSON error: {}", e))?;

    let result = json!({
        "metric": name,
        "expression": metric.expression,
        "description": metric.description,
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
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
            .await
            .expect("attach");

        let json: serde_json::Value = serde_json::from_str(
            &get_describe(&bundle).await.expect("describe should succeed"),
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
            transforms.iter().any(|t| t.as_str().unwrap_or("").contains("ATTACH")),
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
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
            .await
            .expect("attach");

        // Set a column description via the SQL command through the MCP query path.
        execute_query(&bundle, "DESCRIBE COLUMN salary AS 'annual gross pay in USD'")
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
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
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
    async fn test_analyze_combines_describe_and_sample() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
            .await
            .expect("attach");

        let json: serde_json::Value =
            serde_json::from_str(&get_analyze(&bundle, 3).await.expect("analyze")).expect("json");
        // Has both the describe blob and a bounded sample.
        assert!(json["describe"]["columns"].as_array().expect("columns").len() > 1);
        let sample = json["sample"].as_array().expect("sample");
        assert!(sample.len() <= 3 && !sample.is_empty());
    }

    #[tokio::test]
    async fn test_data_quality_reports_nulls_and_distincts() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
            .await
            .expect("attach");

        let json: serde_json::Value = serde_json::from_str(
            &get_data_quality(&bundle).await.expect("data_quality"),
        )
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
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
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
        let col = |n: &str| cols.iter().find(|c| c["name"] == n).expect("column present");

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
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
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
        let col = |n: &str| cols.iter().find(|c| c["name"] == n).expect("column present");

        // "007"/"012" would cast cleanly to BIGINT -- but the padding is meaningful.
        assert!(
            col("code")["suggested_cast"].is_null(),
            "zero-padded identifiers must not be suggested for a numeric cast"
        );
        // A genuine decimal column is still suggested.
        assert_eq!(col("amount")["suggested_cast"], "DOUBLE");
    }

    #[tokio::test]
    async fn test_metric_computes_ungrouped() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
            .await
            .expect("attach");
        execute_query(
            &bundle,
            "CREATE METRIC avg_salary AS 'AVG(salary)' DESCRIPTION 'mean salary in USD'",
        )
        .await
        .expect("create metric");

        let json: serde_json::Value = serde_json::from_str(
            &get_metric(&bundle, "avg_salary", &[], 10).await.expect("metric"),
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
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
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
            assert!(row.get("gender").is_some(), "dimension column present: {row}");
            assert!(row["headcount"].is_number(), "metric present: {row}");
        }
        assert_eq!(json["dimensions"][0], "gender");
    }

    #[tokio::test]
    async fn test_metric_unknown_name_lists_available() {
        use bundlebase_command::BundleBuilderExt;

        let bundle = create_test_bundle().await;
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
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
        let builder = bundle.as_any().downcast_ref::<BundleBuilder>().expect("builder");
        builder
            .attach(bundlebase::test_utils::test_datafile("userdata.parquet"), None)
            .await
            .expect("attach");
        execute_query(&bundle, "CREATE METRIC headcount AS 'COUNT(*)'")
            .await
            .expect("create metric");

        let err = get_metric(&bundle, "headcount", &["not_a_column".to_string()], 10)
            .await
            .expect_err("unknown dimension must error");
        assert!(err.contains("not_a_column"), "names the bad dimension: {err}");
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
