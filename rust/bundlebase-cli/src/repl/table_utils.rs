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

/// Normalize DataFusion's auto-assigned column names for unaliased
/// scalar projections. `SELECT 1` normally projects under the column
/// name `Int64(1)` (DataFusion's debug repr for the literal). For
/// display, peel off the type wrapper so the user sees `1`. Same for
/// `Utf8("hi")` → `"hi"`, `Float64(3.14)` → `3.14`, etc. Plain column
/// references, user-supplied aliases, and compound expressions like
/// `Int64(2) + Int64(2)` pass through unchanged — we only unwrap the
/// case where the *entire* name is a single `Type(...)` token.
fn normalize_header(name: &str) -> String {
    let bytes = name.as_bytes();
    let n = bytes.len();
    if n < 4 || bytes[n - 1] != b')' {
        return name.to_string();
    }
    let Some(open) = name.find('(') else {
        return name.to_string();
    };
    if open == 0 {
        return name.to_string();
    }
    // Prefix must be a plausible Arrow type name: starts uppercase,
    // only ASCII letters / digits.
    let prefix = &name[..open];
    if !prefix
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
    {
        return name.to_string();
    }
    if !prefix.chars().all(|c| c.is_ascii_alphanumeric()) {
        return name.to_string();
    }
    // The first `(` must balance with the *trailing* `)`, i.e. the
    // entire suffix is one parenthesized group. Anything else (e.g.
    // `Int64(2) + Int64(2)`) leaves us mid-expression after the first
    // `)` and we want to skip the rewrite.
    let inner = &name[open + 1..n - 1];
    let mut depth = 1i32;
    for ch in inner.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            // Closed before the trailing `)` — not a single wrapper.
            return name.to_string();
        }
    }
    if depth != 1 {
        return name.to_string();
    }
    inner.to_string()
}

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
                .map(|f| Cell::new(normalize_header(f.name())).fg(Color::Cyan))
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

#[cfg(test)]
mod tests {
    use super::normalize_header;

    #[test]
    fn unwraps_simple_type_wrappers() {
        assert_eq!(normalize_header("Int64(1)"), "1");
        assert_eq!(normalize_header("Float64(3.14)"), "3.14");
        assert_eq!(normalize_header("Utf8(\"hi\")"), "\"hi\"");
        assert_eq!(normalize_header("Boolean(true)"), "true");
    }

    #[test]
    fn passes_through_normal_columns() {
        assert_eq!(normalize_header("count(*)"), "count(*)"); // lowercase prefix
        assert_eq!(normalize_header("project_id"), "project_id");
        assert_eq!(normalize_header("greeting"), "greeting");
        assert_eq!(normalize_header(""), "");
    }

    #[test]
    fn passes_through_compound_expressions() {
        // `Int64(2) + Int64(2)` is *not* a single Type(...) wrapper — the
        // first `(` closes before the trailing `)`. Don't rewrite.
        assert_eq!(
            normalize_header("Int64(2) + Int64(2)"),
            "Int64(2) + Int64(2)"
        );
    }

    #[test]
    fn handles_nested_parens_inside_wrapper() {
        // `Utf8("a(b)c")` is one wrapper around a string that contains
        // its own parens.
        assert_eq!(normalize_header("Utf8(\"a(b)c\")"), "\"a(b)c\"");
    }
}
