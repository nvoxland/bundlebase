//! Stream formatting utilities for REPL display.
//!
//! This module provides functions to format `SendableRecordBatchStream` results
//! based on their expected output shape, producing appropriate terminal output.

use bundlebase_command::OutputShape;
use bundlebase_common::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use futures::StreamExt;

use super::display::format_array_value;
use super::table_utils::format_batches_as_table;

/// Format a record batch stream for terminal display.
///
/// The formatting is adapted based on the shape hint:
/// - `SingleValue`: extracts and returns just the value (no table decoration)
/// - `Dictionary`: formats as "key: value" pairs (1 row, multiple columns)
/// - `Table`: formats as a full table with headers
/// - `None` (regular SQL): always formats as a table
///
/// # Arguments
///
/// * `stream` - The record batch stream to format
/// * `shape_hint` - Optional hint about the expected output shape (from commands)
/// * `limit` - Optional row limit for table output
///
/// # Returns
///
/// Formatted string ready for terminal display.
pub async fn format_stream(
    stream: SendableRecordBatchStream,
    shape_hint: Option<OutputShape>,
    limit: Option<usize>,
) -> Result<String, BundlebaseError> {
    // `None` now means *unlimited* — used by one-shot `bundlebase query`
    // / `bundlebase extend`, where the user already wrote their own LIMIT
    // (or wants the firehose). The interactive REPL still passes
    // `Some(DEFAULT_QUERY_LIMIT)` so terminal users don't accidentally
    // dump a million rows. Previously `None` fell through to the same
    // default, which silently truncated `--format json` output too.
    let limit = limit.unwrap_or(usize::MAX);

    futures::pin_mut!(stream);

    // Collect all batches (commands typically produce small outputs)
    let mut batches = Vec::new();
    while let Some(batch_result) = stream.next().await {
        batches.push(batch_result?);
    }

    if batches.is_empty() {
        return Ok("(0 rows)".to_string());
    }

    // Calculate total rows
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    if total_rows == 0 {
        return Ok("(0 rows)".to_string());
    }

    // Get schema from first batch
    let schema = batches[0].schema();
    let num_cols = schema.fields().len();

    // Determine formatting based on shape hint
    match shape_hint {
        Some(OutputShape::SingleValue) => {
            // Single value: extract and return just the value
            if total_rows == 1 && num_cols == 1 {
                let batch = &batches[0];
                let value = format_array_value(batch.column(0), 0);
                Ok(value)
            } else {
                // Unexpected shape - fall back to table
                format_batches_as_table(&batches, limit)
            }
        }
        Some(OutputShape::Dictionary) => {
            // Dictionary: format as "key: value" pairs
            if total_rows == 1 {
                let batch = &batches[0];
                let mut result = String::new();
                for (i, field) in schema.fields().iter().enumerate() {
                    let value = format_array_value(batch.column(i), 0);
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&format!("{}: {}", field.name(), value));
                }
                Ok(result)
            } else {
                // Multiple rows - fall back to table
                format_batches_as_table(&batches, limit)
            }
        }
        Some(OutputShape::Table) | None => {
            // Table format (default for SQL queries)
            format_batches_as_table(&batches, limit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
    use std::sync::Arc;

    fn create_single_value_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(StringArray::from(vec!["OK"]))],
        )
        .unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    fn create_table_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["Alice", "Bob", "Charlie"])),
            ],
        )
        .unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    #[tokio::test]
    async fn test_format_single_value() {
        let stream = create_single_value_stream();
        let result = format_stream(stream, Some(OutputShape::SingleValue), None)
            .await
            .unwrap();
        assert_eq!(result, "OK");
    }

    #[tokio::test]
    async fn test_format_table() {
        let stream = create_table_stream();
        let result = format_stream(stream, Some(OutputShape::Table), None)
            .await
            .unwrap();
        assert!(result.contains("Alice"));
        assert!(result.contains("Bob"));
        assert!(result.contains("Charlie"));
    }

    fn create_empty_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let stream = futures::stream::iter(Vec::new());
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    #[tokio::test]
    async fn test_format_zero_rows_shows_feedback() {
        // A query that returns no rows must still produce visible output, so
        // the user can tell the query actually ran. Returning an empty string
        // makes the REPL look frozen — see the "pasted SQL doesn't execute"
        // bug report.
        let result = format_stream(create_empty_stream(), Some(OutputShape::Table), None)
            .await
            .unwrap();
        assert_eq!(result, "(0 rows)");
    }

    #[tokio::test]
    async fn test_format_sql_query_as_table() {
        // SQL queries (None shape) should always format as table
        let stream = create_table_stream();
        let result = format_stream(stream, None, None).await.unwrap();
        assert!(result.contains("┌")); // Table border character
    }

    /// Build a stream with `n` rows so we can exercise the limit / no-limit
    /// boundary without dragging in DataFusion.
    fn create_n_row_stream(n: usize) -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let ids: Vec<i64> = (0..n as i64).collect();
        let batch =
            RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(ids))]).unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    /// Regression: passing `None` for the row limit must mean *no limit*,
    /// not "fall back to the REPL default." Before the fix, one-shot
    /// `bundlebase query` calls passed `Some(1000)` and silently truncated
    /// `--format json` output too — corrupting downstream scripts.
    #[tokio::test]
    async fn test_format_stream_none_limit_is_unlimited() {
        let stream = create_n_row_stream(2500);
        let out = format_stream(stream, None, None).await.unwrap();
        // Truncation appends "(output limited to N rows)" — must not appear.
        assert!(
            !out.contains("output limited to"),
            "format_stream(limit=None) silently truncated output:\n{}",
            out.lines().rev().take(3).collect::<Vec<_>>().join("\n")
        );
        // 2500 rows should each show up at least somewhere as a row index.
        // Grep for the last row's id to prove the tail of the result is present.
        assert!(
            out.contains(" 2499 "),
            "expected last row (id=2499) to be in output, but it isn't"
        );
    }

    /// REPL still passes Some(N) for an explicit cap; that path keeps
    /// truncating with the user-facing `(output limited to ...)` footer.
    #[tokio::test]
    async fn test_format_stream_explicit_limit_truncates_with_footer() {
        let stream = create_n_row_stream(2500);
        let out = format_stream(stream, None, Some(50)).await.unwrap();
        assert!(
            out.contains("output limited to 50"),
            "explicit limit must surface to the user via footer, got:\n{}",
            out
        );
    }
}
