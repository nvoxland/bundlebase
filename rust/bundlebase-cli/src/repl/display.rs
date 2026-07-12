//! Display utilities for formatting Arrow arrays as strings.

/// Format an array value at a specific index for display.
///
/// Delegates to Arrow's `ArrayFormatter`, which renders every Arrow type —
/// primitives, strings, dates/times with their real unit and timezone,
/// decimals, dictionaries, run-end encoding, nested types — without any
/// per-type downcasts (the previous hand-rolled match carried 14 bare
/// `unwrap()`s, ADR-007, and printed Debug junk for anything it didn't know).
pub fn format_array_value(column: &arrow::array::ArrayRef, row_idx: usize) -> String {
    if column.is_null(row_idx) {
        return "NULL".to_string();
    }

    match arrow::util::display::ArrayFormatter::try_new(
        column,
        &arrow::util::display::FormatOptions::default(),
    ) {
        Ok(fmt) => fmt.value(row_idx).to_string(),
        // No formatter for this type (shouldn't happen for well-formed
        // arrays) — fall back to the Debug rendering rather than panicking.
        Err(_) => format!("{:?}", column.slice(row_idx, 1)),
    }
}
