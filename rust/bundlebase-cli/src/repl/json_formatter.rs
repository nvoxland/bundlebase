//! JSON formatting utilities for CLI output.
//!
//! This module provides functions to format `SendableRecordBatchStream` results
//! as JSON, suitable for machine consumption in `--format json` mode.

use arrow::array::*;
use arrow::datatypes::DataType;
use bundlebase_command::OutputShape;
use bundlebase_common::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use futures::StreamExt;
use serde_json::{json, Value};

/// Downcast to a concrete array and produce a typed JSON value, falling back
/// to the display string if the concrete type doesn't match the declared
/// DataType (ADR-007: never panic on a malformed array).
macro_rules! typed_json {
    ($col:expr, $row:expr, $arr:ty) => {
        match $col.as_any().downcast_ref::<$arr>() {
            Some(a) => json!(a.value($row)),
            None => json!(super::display::format_array_value($col, $row)),
        }
    };
}

/// Convert an Arrow array value at a given row to a serde_json::Value.
///
/// Produces typed JSON values (numbers as numbers, booleans as booleans, etc.)
fn array_value_to_json(column: &ArrayRef, row_idx: usize) -> Value {
    if column.is_null(row_idx) {
        return Value::Null;
    }

    match column.data_type() {
        DataType::Int8 => typed_json!(column, row_idx, Int8Array),
        DataType::Int16 => typed_json!(column, row_idx, Int16Array),
        DataType::Int32 => typed_json!(column, row_idx, Int32Array),
        DataType::Int64 => typed_json!(column, row_idx, Int64Array),
        DataType::UInt8 => typed_json!(column, row_idx, UInt8Array),
        DataType::UInt16 => typed_json!(column, row_idx, UInt16Array),
        DataType::UInt32 => typed_json!(column, row_idx, UInt32Array),
        DataType::UInt64 => typed_json!(column, row_idx, UInt64Array),
        DataType::Float32 => {
            let Some(arr) = column.as_any().downcast_ref::<Float32Array>() else {
                return json!(super::display::format_array_value(column, row_idx));
            };
            let v = arr.value(row_idx);
            if v.is_finite() {
                json!(v)
            } else {
                json!(v.to_string())
            }
        }
        DataType::Float64 => {
            let Some(arr) = column.as_any().downcast_ref::<Float64Array>() else {
                return json!(super::display::format_array_value(column, row_idx));
            };
            let v = arr.value(row_idx);
            if v.is_finite() {
                json!(v)
            } else {
                json!(v.to_string())
            }
        }
        DataType::Boolean => typed_json!(column, row_idx, BooleanArray),
        DataType::Utf8 => typed_json!(column, row_idx, StringArray),
        DataType::LargeUtf8 => typed_json!(column, row_idx, LargeStringArray),
        DataType::Utf8View => typed_json!(column, row_idx, StringViewArray),
        // Date / Time / Timestamp / Duration / Interval / Decimal all go
        // through display::format_array_value, which delegates to Arrow's
        // ArrayFormatter for unit-aware ISO-8601 rendering. Previously
        // Date64 emitted the raw epoch-millis int and Timestamp ignored
        // the array's unit (always assumed nanosecond).
        DataType::Date32
        | DataType::Date64
        | DataType::Time32(_)
        | DataType::Time64(_)
        | DataType::Timestamp(_, _)
        | DataType::Duration(_)
        | DataType::Interval(_)
        | DataType::Decimal128(_, _)
        | DataType::Decimal256(_, _) => {
            let formatted = super::display::format_array_value(column, row_idx);
            json!(formatted)
        }
        _ => {
            // Fallback: use string representation
            let formatted = super::display::format_array_value(column, row_idx);
            json!(formatted)
        }
    }
}

/// Format a record batch stream as JSON.
///
/// The formatting is adapted based on the shape hint:
/// - `SingleValue`: returns a single JSON value
/// - `Dictionary`: returns a single JSON object
/// - `Table` or `None`: returns a JSON array of objects
///
/// # Arguments
///
/// * `stream` - The record batch stream to format
/// * `shape_hint` - Optional hint about the expected output shape
/// * `limit` - Optional row limit
///
/// # Returns
///
/// JSON string ready for output.
pub async fn format_stream_json(
    stream: SendableRecordBatchStream,
    shape_hint: Option<OutputShape>,
    limit: Option<usize>,
) -> Result<String, BundlebaseError> {
    // `None` = unlimited. JSON output is for piping into scripts, where a
    // silent truncation produces wrong analyses with no error. Callers
    // that want a cap (e.g. interactive REPL) pass it explicitly.
    let limit = limit.unwrap_or(usize::MAX);

    futures::pin_mut!(stream);

    // Collect all batches
    let mut batches = Vec::new();
    while let Some(batch_result) = stream.next().await {
        batches.push(batch_result?);
    }

    if batches.is_empty() {
        return match shape_hint {
            Some(OutputShape::SingleValue) => Ok("null".to_string()),
            Some(OutputShape::Dictionary) => Ok("{}".to_string()),
            _ => Ok("[]".to_string()),
        };
    }

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    if total_rows == 0 {
        return match shape_hint {
            Some(OutputShape::SingleValue) => Ok("null".to_string()),
            Some(OutputShape::Dictionary) => Ok("{}".to_string()),
            _ => Ok("[]".to_string()),
        };
    }

    let schema = batches[0].schema();

    match shape_hint {
        Some(OutputShape::SingleValue) => {
            if total_rows == 1 && schema.fields().len() == 1 {
                let batch = &batches[0];
                let value = array_value_to_json(batch.column(0), 0);
                Ok(serde_json::to_string_pretty(&value).map_err(|e| {
                    BundlebaseError::from(format!("JSON serialization error: {}", e))
                })?)
            } else {
                // Unexpected shape - fall back to array
                format_as_array(&batches, &schema, limit)
            }
        }
        Some(OutputShape::Dictionary) => {
            if total_rows == 1 {
                let batch = &batches[0];
                let mut obj = serde_json::Map::new();
                for (i, field) in schema.fields().iter().enumerate() {
                    obj.insert(
                        field.name().clone(),
                        array_value_to_json(batch.column(i), 0),
                    );
                }
                Ok(
                    serde_json::to_string_pretty(&Value::Object(obj)).map_err(|e| {
                        BundlebaseError::from(format!("JSON serialization error: {}", e))
                    })?,
                )
            } else {
                // Multiple rows - fall back to array
                format_as_array(&batches, &schema, limit)
            }
        }
        Some(OutputShape::Table) | None => {
            let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
            let result = format_as_array(&batches, &schema, limit)?;
            if total_rows >= limit {
                eprintln!("(output limited to {} rows)", limit);
            }
            Ok(result)
        }
    }
}

/// Format batches as a JSON array of objects.
fn format_as_array(
    batches: &[arrow::record_batch::RecordBatch],
    schema: &arrow_schema::SchemaRef,
    limit: usize,
) -> Result<String, BundlebaseError> {
    let mut rows = Vec::new();
    let mut row_count = 0;

    for batch in batches {
        for row_idx in 0..batch.num_rows() {
            if row_count >= limit {
                break;
            }
            let mut obj = serde_json::Map::new();
            for (col_idx, field) in schema.fields().iter().enumerate() {
                obj.insert(
                    field.name().clone(),
                    array_value_to_json(batch.column(col_idx), row_idx),
                );
            }
            rows.push(Value::Object(obj));
            row_count += 1;
        }
        if row_count >= limit {
            break;
        }
    }

    Ok(serde_json::to_string_pretty(&rows)
        .map_err(|e| BundlebaseError::from(format!("JSON serialization error: {}", e)))?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_dictionary_column_formats_without_panic() {
        // bb-8gq.8: dictionary/REE columns previously fell through to Debug
        // output in the display path and were a panic risk in refactors; the
        // ArrayFormatter delegation renders their VALUES.
        use arrow::array::{DictionaryArray, Int32Array, StringArray};
        use std::sync::Arc;

        let keys = Int32Array::from(vec![0, 1, 0]);
        let values = StringArray::from(vec!["alpha", "beta"]);
        let dict = DictionaryArray::new(keys, Arc::new(values));
        let col: ArrayRef = Arc::new(dict);

        let display = super::super::display::format_array_value(&col, 1);
        assert_eq!(display, "beta");

        let j = array_value_to_json(&col, 1);
        assert_eq!(j, serde_json::json!("beta"));
    }

    use super::*;
    use arrow::datatypes::{Field, Schema};
    use arrow::record_batch::RecordBatch;
    use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
    use std::sync::Arc;

    fn create_single_value_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "count",
            DataType::Int64,
            false,
        )]));
        let batch =
            RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(vec![42]))])
                .unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    fn create_table_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("active", DataType::Boolean, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["Alice", "Bob", "Charlie"])),
                Arc::new(BooleanArray::from(vec![true, false, true])),
            ],
        )
        .unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    fn create_empty_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let stream = futures::stream::iter(vec![]);
        Box::pin(RecordBatchStreamAdapter::new(schema, stream))
    }

    #[tokio::test]
    async fn test_json_single_value() {
        let stream = create_single_value_stream();
        let result = format_stream_json(stream, Some(OutputShape::SingleValue), None)
            .await
            .unwrap();
        assert_eq!(result, "42");
    }

    #[tokio::test]
    async fn test_json_table() {
        let stream = create_table_stream();
        let result = format_stream_json(stream, Some(OutputShape::Table), None)
            .await
            .unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0]["id"], 1);
        assert_eq!(parsed[0]["name"], "Alice");
        assert_eq!(parsed[0]["active"], true);
        assert_eq!(parsed[1]["name"], "Bob");
        assert_eq!(parsed[2]["name"], "Charlie");
    }

    #[tokio::test]
    async fn test_json_dictionary() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("version", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["my_bundle"])),
                Arc::new(StringArray::from(vec!["v1"])),
            ],
        )
        .unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        let stream: SendableRecordBatchStream =
            Box::pin(RecordBatchStreamAdapter::new(schema, stream));

        let result = format_stream_json(stream, Some(OutputShape::Dictionary), None)
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["name"], "my_bundle");
        assert_eq!(parsed["version"], "v1");
    }

    #[tokio::test]
    async fn test_json_empty_stream() {
        let stream = create_empty_stream();
        let result = format_stream_json(stream, Some(OutputShape::Table), None)
            .await
            .unwrap();
        assert_eq!(result, "[]");
    }

    #[tokio::test]
    async fn test_json_empty_single_value() {
        let stream = create_empty_stream();
        let result = format_stream_json(stream, Some(OutputShape::SingleValue), None)
            .await
            .unwrap();
        assert_eq!(result, "null");
    }

    #[tokio::test]
    async fn test_json_limit() {
        let stream = create_table_stream();
        let result = format_stream_json(stream, Some(OutputShape::Table), Some(2))
            .await
            .unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[tokio::test]
    async fn test_json_null_values() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("name", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), None])),
                Arc::new(StringArray::from(vec![None, Some("Bob")])),
            ],
        )
        .unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        let stream: SendableRecordBatchStream =
            Box::pin(RecordBatchStreamAdapter::new(schema, stream));

        let result = format_stream_json(stream, None, None).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed[0]["id"], 1);
        assert!(parsed[0]["name"].is_null());
        assert!(parsed[1]["id"].is_null());
        assert_eq!(parsed[1]["name"], "Bob");
    }

    /// Regression: JSON output must not silently truncate when limit=None.
    /// Before the fix, `bundlebase query --format json "SELECT … LIMIT 50000"`
    /// returned only 1000 rows even though the SQL asked for 50k.
    #[tokio::test]
    async fn test_format_stream_json_none_limit_emits_all_rows() {
        let n = 2500;
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let ids: Vec<i64> = (0..n as i64).collect();
        let batch =
            RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(ids))]).unwrap();
        let stream = futures::stream::iter(vec![Ok(batch)]);
        let stream: SendableRecordBatchStream =
            Box::pin(RecordBatchStreamAdapter::new(schema, stream));

        let result = format_stream_json(stream, None, None).await.unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed.len(),
            n,
            "expected {} JSON rows when limit=None, got {}",
            n,
            parsed.len()
        );
    }
}
