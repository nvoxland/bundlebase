use arrow_schema::SchemaRef;
use bundlebase::{bundle::BundleCommit, AnyOperation, BundlebaseError, Operation};
use comfy_table::{presets::UTF8_FULL, Cell, Color, ContentArrangement, Table};
use datafusion::prelude::DataFrame;
use futures::StreamExt;
use std::sync::Arc;

/// Display a DataFrame as a formatted table
pub async fn display_dataframe(
    df: &Arc<DataFrame>,
    limit: Option<usize>,
) -> Result<String, BundlebaseError> {
    let limit = limit.unwrap_or(10);
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);

    let stream: datafusion::execution::SendableRecordBatchStream =
        df.as_ref().clone().execute_stream().await?;
    let mut row_count = 0;

    futures::pin_mut!(stream);

    while let Some(batch) = stream.next().await {
        let batch = batch?;

        // Add header on first batch
        if row_count == 0 {
            let header: Vec<Cell> = batch
                .schema()
                .fields()
                .iter()
                .map(|f| Cell::new(f.name()).fg(Color::Cyan))
                .collect();
            table.set_header(header);
        }

        // Add rows
        for row_idx in 0..batch.num_rows() {
            if row_count >= limit {
                break;
            }

            let row: Vec<Cell> = (0..batch.num_columns())
                .map(|col_idx| {
                    let column = batch.column(col_idx);
                    let value = format_array_value(column, row_idx);
                    Cell::new(value)
                })
                .collect();

            table.add_row(row);
            row_count += 1;
        }

        if row_count >= limit {
            break;
        }
    }

    if row_count == 0 {
        Ok("No rows to display".to_string())
    } else {
        let mut output = table.to_string();
        if row_count >= limit {
            output.push_str(&format!("\n(Showing first {} rows)", limit));
        }
        Ok(output)
    }
}

/// Display schema as a formatted table
pub fn display_schema(schema: SchemaRef) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);

    table.set_header(vec![
        Cell::new("Column").fg(Color::Cyan),
        Cell::new("Type").fg(Color::Cyan),
        Cell::new("Nullable").fg(Color::Cyan),
    ]);

    for field in schema.fields() {
        table.add_row(vec![
            Cell::new(field.name()),
            Cell::new(field.data_type().to_string()),
            Cell::new(if field.is_nullable() { "Yes" } else { "No" }),
        ]);
    }

    if schema.fields().is_empty() {
        "No columns in schema".to_string()
    } else {
        table.to_string()
    }
}

pub fn display_status(new_ops: Vec<AnyOperation>) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);

    for op in new_ops {
        table.add_row(vec![Cell::new(&op.describe())]);
    }

    if table.is_empty() {
        "No commit history".to_string()
    } else {
        table.to_string()
    }
}

/// Display commit history as a formatted table
pub fn display_history(commits: Vec<BundleCommit>) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);

    table.set_header(vec![
        Cell::new("Timestamp").fg(Color::Cyan),
        Cell::new("Author").fg(Color::Cyan),
        Cell::new("Message").fg(Color::Cyan),
    ]);

    for commit in commits {
        table.add_row(vec![
            Cell::new(&commit.timestamp),
            Cell::new(&commit.author),
            Cell::new(&commit.message),
        ]);
    }

    if table.is_empty() {
        "No commit history".to_string()
    } else {
        table.to_string()
    }
}

/// Format an array value at a specific index for display
fn format_array_value(column: &arrow::array::ArrayRef, row_idx: usize) -> String {
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
