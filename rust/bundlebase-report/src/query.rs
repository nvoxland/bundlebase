//! Bounded query execution for report blocks.
//!
//! Executes SQL queries against bundles with a hard row limit,
//! returning results as column names + JSON-compatible row values.

use arrow::array::*;
use arrow::datatypes::DataType;
use arrow::util::display::ArrayFormatter;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use futures::StreamExt;
use std::sync::Arc;

use crate::MAX_TABLE_ROWS;

/// Query result bounded to [`MAX_TABLE_ROWS`].
#[derive(Debug, Clone)]
pub struct BoundedQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

/// Execute a query against a bundle, collecting up to [`MAX_TABLE_ROWS`] rows.
pub async fn execute_bounded_query(
    bundle: &Arc<dyn BundleFacade>,
    sql: &str,
) -> Result<BoundedQueryResult, BundlebaseError> {
    let mut stream = bundle.query(sql, vec![], Some(MAX_TABLE_ROWS)).await?;
    let schema = {
        use datafusion::physical_plan::RecordBatchStream;
        RecordBatchStream::schema(stream.as_ref().get_ref())
    };

    let columns: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    let mut rows = Vec::new();

    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        for row_idx in 0..batch.num_rows() {
            let mut row = Vec::with_capacity(batch.num_columns());
            for col_idx in 0..batch.num_columns() {
                let value = extract_value(batch.column(col_idx).as_ref(), row_idx);
                row.push(value);
            }
            rows.push(row);
        }
    }

    Ok(BoundedQueryResult { columns, rows })
}

/// Extract a JSON-compatible value from an Arrow array at the given row index.
fn extract_value(array: &dyn Array, row: usize) -> serde_json::Value {
    if array.is_null(row) {
        return serde_json::Value::Null;
    }

    match array.data_type() {
        DataType::Boolean => {
            if let Some(arr) = array.as_any().downcast_ref::<BooleanArray>() {
                serde_json::Value::Bool(arr.value(row))
            } else {
                serde_json::Value::Null
            }
        }
        DataType::Int8 => extract_int::<Int8Type>(array, row),
        DataType::Int16 => extract_int::<Int16Type>(array, row),
        DataType::Int32 => extract_int::<Int32Type>(array, row),
        DataType::Int64 => extract_int::<Int64Type>(array, row),
        DataType::UInt8 => extract_uint::<UInt8Type>(array, row),
        DataType::UInt16 => extract_uint::<UInt16Type>(array, row),
        DataType::UInt32 => extract_uint::<UInt32Type>(array, row),
        DataType::UInt64 => extract_uint::<UInt64Type>(array, row),
        DataType::Float32 => {
            if let Some(arr) = array.as_any().downcast_ref::<Float32Array>() {
                serde_json::json!(arr.value(row))
            } else {
                serde_json::Value::Null
            }
        }
        DataType::Float64 => {
            if let Some(arr) = array.as_any().downcast_ref::<Float64Array>() {
                serde_json::json!(arr.value(row))
            } else {
                serde_json::Value::Null
            }
        }
        DataType::Utf8 => {
            if let Some(arr) = array.as_any().downcast_ref::<StringArray>() {
                serde_json::Value::String(arr.value(row).to_string())
            } else {
                serde_json::Value::Null
            }
        }
        DataType::LargeUtf8 => {
            if let Some(arr) = array.as_any().downcast_ref::<LargeStringArray>() {
                serde_json::Value::String(arr.value(row).to_string())
            } else {
                serde_json::Value::Null
            }
        }
        _ => {
            // Fallback: use Arrow's display formatter
            match ArrayFormatter::try_new(array, &Default::default()) {
                Ok(f) => serde_json::Value::String(f.value(row).to_string()),
                Err(_) => serde_json::Value::Null,
            }
        }
    }
}

use arrow::datatypes::*;

fn extract_int<T: ArrowPrimitiveType>(array: &dyn Array, row: usize) -> serde_json::Value
where
    T::Native: Into<i64>,
{
    if let Some(arr) = array.as_any().downcast_ref::<PrimitiveArray<T>>() {
        let val: i64 = arr.value(row).into();
        serde_json::json!(val)
    } else {
        serde_json::Value::Null
    }
}

fn extract_uint<T: ArrowPrimitiveType>(array: &dyn Array, row: usize) -> serde_json::Value
where
    T::Native: Into<u64>,
{
    if let Some(arr) = array.as_any().downcast_ref::<PrimitiveArray<T>>() {
        let val: u64 = arr.value(row).into();
        serde_json::json!(val)
    } else {
        serde_json::Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_string_value() {
        let array = StringArray::from(vec!["hello", "world"]);
        assert_eq!(extract_value(&array, 0), serde_json::json!("hello"));
        assert_eq!(extract_value(&array, 1), serde_json::json!("world"));
    }

    #[test]
    fn test_extract_int_value() {
        let array = Int64Array::from(vec![42, -7]);
        assert_eq!(extract_value(&array, 0), serde_json::json!(42));
        assert_eq!(extract_value(&array, 1), serde_json::json!(-7));
    }

    #[test]
    fn test_extract_float_value() {
        let array = Float64Array::from(vec![3.14, 2.71]);
        assert_eq!(extract_value(&array, 0), serde_json::json!(3.14));
    }

    #[test]
    fn test_extract_bool_value() {
        let array = BooleanArray::from(vec![true, false]);
        assert_eq!(extract_value(&array, 0), serde_json::json!(true));
        assert_eq!(extract_value(&array, 1), serde_json::json!(false));
    }

    #[test]
    fn test_extract_null_value() {
        let array = StringArray::from(vec![Some("hello"), None]);
        assert_eq!(extract_value(&array, 1), serde_json::Value::Null);
    }
}
