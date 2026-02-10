//! Shared table formatting utilities for REPL display.
//!
//! This module provides common table formatting functions used by both
//! display.rs and stream_formatter.rs.

use arrow::record_batch::RecordBatch;
use bundlebase::BundlebaseError;
use comfy_table::{presets::UTF8_FULL, Cell, Color, ContentArrangement, Table};

use super::display::format_array_value;

/// Default row limit for SQL query results.
pub const DEFAULT_QUERY_LIMIT: usize = 100;

/// Format record batches as a table.
///
/// This is the shared implementation used by both streaming and batch display.
///
/// # Arguments
///
/// * `batches` - The record batches to format
/// * `limit` - Maximum number of rows to display
///
/// # Returns
///
/// Formatted table string ready for terminal display.
pub fn format_batches_as_table(
    batches: &[RecordBatch],
    limit: usize,
) -> Result<String, BundlebaseError> {
    if batches.is_empty() {
        return Ok("No rows to display".to_string());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);

    let mut row_count = 0;

    for (batch_idx, batch) in batches.iter().enumerate() {
        // Add header on first batch
        if batch_idx == 0 {
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
