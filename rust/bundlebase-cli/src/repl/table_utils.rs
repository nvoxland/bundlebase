//! Shared table formatting utilities for REPL display.
//!
//! This module provides common table formatting functions used by both
//! display.rs and stream_formatter.rs.

use arrow::record_batch::RecordBatch;
use bundlebase_common::BundlebaseError;
use comfy_table::{presets::UTF8_FULL, Cell, Color, ContentArrangement, Table};

use super::display::format_array_value;

/// Default row limit for SQL query results.
pub const DEFAULT_QUERY_LIMIT: usize = 100;

/// Per-cell character cap. Long string cells (paragraph-long
/// `content_text`, JSON blobs, escape-laden tool output) are truncated
/// to this length with an `…` suffix before being handed to comfy-table.
/// Without it, even `SELECT * FROM bundle LIMIT 5` on a wide schema
/// produced ~50 KB of output for 5 rows because each cell padded to its
/// longest value.
const MAX_CELL_CHARS: usize = 80;

/// Truncate cell text to keep table output sane on wide-string columns.
fn truncate_cell(s: &str) -> String {
    // Single-line first — newlines explode line count and rarely
    // survive a row-oriented terminal display anyway.
    let single_line = s.replace('\n', "\\n");
    if single_line.chars().count() <= MAX_CELL_CHARS {
        single_line
    } else {
        let mut truncated: String = single_line.chars().take(MAX_CELL_CHARS).collect();
        truncated.push('…');
        truncated
    }
}

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
    // Use the simpler `Disabled` arrangement: each cell renders as-is
    // (after the per-cell cap below trims paragraph-long values). The
    // Dynamic arrangement was problematic with 50+ column schemas — it
    // would either pad each cell to its longest value (massive output
    // when piped, no terminal width to anchor against) or wrap each
    // cell across many lines (also huge). For the common case the cell
    // cap alone is enough to keep output bounded.
    table.set_content_arrangement(ContentArrangement::Disabled);

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
                    Cell::new(truncate_cell(&value))
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
            output.push_str(&format!("\n(output limited to {} rows)", limit));
        }
        Ok(output)
    }
}
