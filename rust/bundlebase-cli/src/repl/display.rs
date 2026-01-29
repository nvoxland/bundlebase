//! Display utilities for formatting Arrow arrays as strings.

/// Format an array value at a specific index for display
pub fn format_array_value(column: &arrow::array::ArrayRef, row_idx: usize) -> String {
    use arrow::array::*;
    use arrow::datatypes::DataType;

    if column.is_null(row_idx) {
        return "NULL".to_string();
    }

    match column.data_type() {
        DataType::Int8 => column
            .as_any()
            .downcast_ref::<Int8Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Int16 => column
            .as_any()
            .downcast_ref::<Int16Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Int32 => column
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Int64 => column
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::UInt8 => column
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::UInt16 => column
            .as_any()
            .downcast_ref::<UInt16Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::UInt32 => column
            .as_any()
            .downcast_ref::<UInt32Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::UInt64 => column
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Float32 => column
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Float64 => column
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Utf8 => column
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::LargeUtf8 => column
            .as_any()
            .downcast_ref::<LargeStringArray>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Utf8View => column
            .as_any()
            .downcast_ref::<StringViewArray>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Boolean => column
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap()
            .value(row_idx)
            .to_string(),
        DataType::Date32 => {
            let value = column
                .as_any()
                .downcast_ref::<Date32Array>()
                .unwrap()
                .value(row_idx);
            format!("Date32({})", value)
        }
        DataType::Date64 => {
            let value = column
                .as_any()
                .downcast_ref::<Date64Array>()
                .unwrap()
                .value(row_idx);
            format!("Date64({})", value)
        }
        DataType::Timestamp(unit, tz) => {
            let value = column
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .map(|arr| arr.value(row_idx))
                .unwrap_or(0);
            format!("Timestamp({:?}, {:?})({})", unit, tz, value)
        }
        _ => format!("{:?}", column.slice(row_idx, 1)),
    }
}
