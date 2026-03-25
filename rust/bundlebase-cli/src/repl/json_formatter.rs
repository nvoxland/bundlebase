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

/// Convert an Arrow array value at a given row to a serde_json::Value.
///
/// Produces typed JSON values (numbers as numbers, booleans as booleans, etc.)
fn array_value_to_json(column: &ArrayRef, row_idx: usize) -> Value {
    if column.is_null(row_idx) {
        return Value::Null;
    }

    match column.data_type() {
        DataType::Int8 => json!(column.as_any().downcast_ref::<Int8Array>().expect("Int8 downcast").value(row_idx)),
        DataType::Int16 => json!(column.as_any().downcast_ref::<Int16Array>().expect("Int16 downcast").value(row_idx)),
        DataType::Int32 => json!(column.as_any().downcast_ref::<Int32Array>().expect("Int32 downcast").value(row_idx)),
        DataType::Int64 => json!(column.as_any().downcast_ref::<Int64Array>().expect("Int64 downcast").value(row_idx)),
        DataType::UInt8 => json!(column.as_any().downcast_ref::<UInt8Array>().expect("UInt8 downcast").value(row_idx)),
        DataType::UInt16 => json!(column.as_any().downcast_ref::<UInt16Array>().expect("UInt16 downcast").value(row_idx)),
        DataType::UInt32 => json!(column.as_any().downcast_ref::<UInt32Array>().expect("UInt32 downcast").value(row_idx)),
        DataType::UInt64 => json!(column.as_any().downcast_ref::<UInt64Array>().expect("UInt64 downcast").value(row_idx)),
        DataType::Float32 => {
            let v = column.as_any().downcast_ref::<Float32Array>().expect("Float32 downcast").value(row_idx);
            if v.is_finite() { json!(v) } else { json!(v.to_string()) }
        }
        DataType::Float64 => {
            let v = column.as_any().downcast_ref::<Float64Array>().expect("Float64 downcast").value(row_idx);
            if v.is_finite() { json!(v) } else { json!(v.to_string()) }
        }
        DataType::Boolean => json!(column.as_any().downcast_ref::<BooleanArray>().expect("Boolean downcast").value(row_idx)),
        DataType::Utf8 => json!(column.as_any().downcast_ref::<StringArray>().expect("Utf8 downcast").value(row_idx)),
        DataType::LargeUtf8 => json!(column.as_any().downcast_ref::<LargeStringArray>().expect("LargeUtf8 downcast").value(row_idx)),
        DataType::Utf8View => json!(column.as_any().downcast_ref::<StringViewArray>().expect("Utf8View downcast").value(row_idx)),
        DataType::Date32 => {
            // Use Arrow's display formatting for dates
            let formatted = super::display::format_array_value(column, row_idx);
            json!(formatted)
        }
        DataType::Date64 => {
            let v = column.as_any().downcast_ref::<Date64Array>().expect("Date64 downcast").value(row_idx);
            json!(v)
        }
        DataType::Timestamp(_, _) => {
            // Use Arrow's display formatting for timestamps
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
    let limit = limit.unwrap_or(1000);

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
                Ok(serde_json::to_string_pretty(&value)
                    .map_err(|e| BundlebaseError::from(format!("JSON serialization error: {}", e)))?)
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
                Ok(serde_json::to_string_pretty(&Value::Object(obj))
                    .map_err(|e| BundlebaseError::from(format!("JSON serialization error: {}", e)))?)
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
    use super::*;
    use arrow::datatypes::{Field, Schema};
    use arrow::record_batch::RecordBatch;
    use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
    use std::sync::Arc;

    fn create_single_value_stream() -> SendableRecordBatchStream {
        let schema = Arc::new(Schema::new(vec![Field::new("count", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![42]))],
        )
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
}
