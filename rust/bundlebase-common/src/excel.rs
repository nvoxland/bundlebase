//! Excel file conversion utilities.
//!
//! Converts Excel files (.xlsx, .xls, .ods) to Parquet format for ingestion.
//! All cell values are read as strings (Utf8) — use CAST COLUMN after
//! attaching to convert to specific types.

use crate::BundlebaseError;
use arrow::array::{ArrayRef, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use bytes::Bytes;
use calamine::{open_workbook_auto_from_rs, Data, Reader};
use std::io::Cursor;
use std::sync::Arc;

/// Check if a filename has an Excel extension.
pub fn is_excel_format(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".xlsx") || lower.ends_with(".xls") || lower.ends_with(".ods")
}

/// Convert Excel file bytes to Parquet bytes.
///
/// Reads the first (or named) sheet, treats the first row as headers,
/// and converts all cell values to strings. Returns Parquet bytes suitable
/// for writing to the data directory.
pub fn excel_to_parquet(data: &[u8], sheet_name: Option<&str>) -> Result<Bytes, BundlebaseError> {
    let cursor = Cursor::new(data);
    let mut workbook = open_workbook_auto_from_rs(cursor)
        .map_err(|e| BundlebaseError::from(format!("Failed to open Excel file: {}", e)))?;

    let sheet_names = workbook.sheet_names().to_vec();
    if sheet_names.is_empty() {
        return Err("Excel file contains no sheets".into());
    }

    let target_sheet = match sheet_name {
        Some(name) => {
            if !sheet_names.contains(&name.to_string()) {
                return Err(BundlebaseError::from(format!(
                    "Sheet '{}' not found. Available sheets: {}",
                    name,
                    sheet_names.join(", ")
                )));
            }
            name.to_string()
        }
        None => sheet_names[0].clone(),
    };

    let range = workbook.worksheet_range(&target_sheet).map_err(|e| {
        BundlebaseError::from(format!("Failed to read sheet '{}': {}", target_sheet, e))
    })?;

    let (row_count, col_count) = range.get_size();
    if row_count == 0 || col_count == 0 {
        return Err(BundlebaseError::from(format!(
            "Sheet '{}' is empty",
            target_sheet
        )));
    }

    // First row = headers
    let headers: Vec<String> = (0..col_count)
        .map(|c| {
            range
                .get((0, c))
                .map(|v| cell_to_string(v))
                .unwrap_or_else(|| format!("column_{}", c))
        })
        .collect();

    // Build string arrays for each column
    let mut builders: Vec<StringBuilder> = (0..col_count).map(|_| StringBuilder::new()).collect();

    for row in 1..row_count {
        for col in 0..col_count {
            match range.get((row, col)) {
                Some(cell) if !matches!(cell, Data::Empty) => {
                    builders[col].append_value(cell_to_string(cell));
                }
                _ => {
                    builders[col].append_null();
                }
            }
        }
    }

    let fields: Vec<Field> = headers
        .iter()
        .map(|name| Field::new(name, DataType::Utf8, true))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let arrays: Vec<ArrayRef> = builders
        .into_iter()
        .map(|mut b| Arc::new(b.finish()) as ArrayRef)
        .collect();

    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|e| BundlebaseError::from(format!("Failed to create RecordBatch: {}", e)))?;

    // Convert to Parquet
    let mut buffer = Vec::new();
    {
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::ZSTD(
                parquet::basic::ZstdLevel::try_new(3)
                    .unwrap_or(parquet::basic::ZstdLevel::default()),
            ))
            .build();
        let mut writer = parquet::arrow::ArrowWriter::try_new(&mut buffer, schema, Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
    }

    Ok(Bytes::from(buffer))
}

/// Convert an Excel cell value to a string.
fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if *f == (*f as i64) as f64 && f.is_finite() {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::String(s) => s.clone(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => format!("{}", dt.as_f64()),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERROR:{:?}", e),
        Data::Empty => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_excel_format() {
        assert!(is_excel_format("data.xlsx"));
        assert!(is_excel_format("data.XLSX"));
        assert!(is_excel_format("data.xls"));
        assert!(is_excel_format("data.ods"));
        assert!(!is_excel_format("data.csv"));
        assert!(!is_excel_format("data.parquet"));
    }

    #[test]
    fn test_cell_to_string() {
        assert_eq!(cell_to_string(&Data::Int(42)), "42");
        assert_eq!(cell_to_string(&Data::Float(3.14)), "3.14");
        assert_eq!(cell_to_string(&Data::Float(100.0)), "100");
        assert_eq!(cell_to_string(&Data::String("hello".to_string())), "hello");
        assert_eq!(cell_to_string(&Data::Bool(true)), "true");
        assert_eq!(cell_to_string(&Data::Empty), "");
    }
}
