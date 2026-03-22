//! Unified export writer system for writing query results to files.
//!
//! This module provides an extensible `ExportWriter` trait with implementations
//! for CSV and JSON Lines formats. Format is determined by file extension.
//!
//! # Usage
//!
//! ```ignore
//! let mut writer = create_export_writer("output.csv", &schema)?;
//! for batch in batches {
//!     writer.write_batch(&batch)?;
//! }
//! let row_count = writer.finish()?;
//! ```

use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use crate::BundlebaseError;

/// Trait for writing record batches to an output sink.
///
/// Implementations handle format-specific serialization (CSV, JSON Lines, etc.).
/// Writers are created via [`create_export_writer`] which selects the format
/// based on file extension.
pub trait ExportWriter: Send {
    /// Write a single record batch to the output.
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), BundlebaseError>;

    /// Finalize the output and return the total number of rows written.
    ///
    /// This must be called after all batches have been written to ensure
    /// the output is complete (e.g., flushing buffers).
    fn finish(self: Box<Self>) -> Result<usize, BundlebaseError>;
}

/// Supported export formats, determined by file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    JsonLines,
}

impl ExportFormat {
    /// Determine the export format from a file path's extension.
    ///
    /// Supported extensions: `.csv`, `.jsonl`
    pub fn from_path(path: &str) -> Result<Self, BundlebaseError> {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());

        match ext.as_deref() {
            Some("csv") => Ok(ExportFormat::Csv),
            Some("jsonl") => Ok(ExportFormat::JsonLines),
            Some(other) => Err(BundlebaseError::from(format!(
                "Unsupported export format '.{}'. Supported: .csv, .jsonl",
                other
            ))),
            None => Err(BundlebaseError::from(
                "Cannot determine export format: file has no extension. Supported: .csv, .jsonl",
            )),
        }
    }
}

/// Create an export writer for the given file path.
///
/// The format is determined by the file extension. The file is created (or truncated
/// if it already exists).
pub fn create_export_writer(
    path: &str,
    schema: &SchemaRef,
) -> Result<Box<dyn ExportWriter>, BundlebaseError> {
    let format = ExportFormat::from_path(path)?;
    let file = File::create(path).map_err(|e| {
        BundlebaseError::from(format!("Failed to create export file '{}': {}", path, e))
    })?;
    let buf_writer = BufWriter::new(file);

    match format {
        ExportFormat::Csv => Ok(Box::new(CsvExportWriter::new(buf_writer)?)),
        ExportFormat::JsonLines => Ok(Box::new(JsonLinesExportWriter::new(buf_writer, schema)?)),
    }
}

// ============================================================================
// CSV Export Writer
// ============================================================================

struct CsvExportWriter {
    writer: arrow::csv::Writer<BufWriter<File>>,
    row_count: usize,
}

impl CsvExportWriter {
    fn new(writer: BufWriter<File>) -> Result<Self, BundlebaseError> {
        let csv_writer = arrow::csv::WriterBuilder::new()
            .with_header(true)
            .build(writer);
        Ok(Self {
            writer: csv_writer,
            row_count: 0,
        })
    }
}

impl ExportWriter for CsvExportWriter {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), BundlebaseError> {
        self.row_count += batch.num_rows();
        self.writer.write(batch).map_err(|e| {
            BundlebaseError::from(format!("Failed to write CSV batch: {}", e))
        })
    }

    fn finish(self: Box<Self>) -> Result<usize, BundlebaseError> {
        Ok(self.row_count)
    }
}

// ============================================================================
// JSON Lines Export Writer
// ============================================================================

struct JsonLinesExportWriter {
    writer: arrow::json::LineDelimitedWriter<BufWriter<File>>,
    row_count: usize,
}

impl JsonLinesExportWriter {
    fn new(writer: BufWriter<File>, _schema: &SchemaRef) -> Result<Self, BundlebaseError> {
        let json_writer = arrow::json::LineDelimitedWriter::new(writer);
        Ok(Self {
            writer: json_writer,
            row_count: 0,
        })
    }
}

impl ExportWriter for JsonLinesExportWriter {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), BundlebaseError> {
        self.row_count += batch.num_rows();
        self.writer.write(batch).map_err(|e| {
            BundlebaseError::from(format!("Failed to write JSON Lines batch: {}", e))
        })
    }

    fn finish(mut self: Box<Self>) -> Result<usize, BundlebaseError> {
        let row_count = self.row_count;
        self.writer.finish().map_err(|e| {
            BundlebaseError::from(format!("Failed to finish JSON Lines output: {}", e))
        })?;
        Ok(row_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int32Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn test_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]))
    }

    fn test_batch() -> RecordBatch {
        RecordBatch::try_new(
            test_schema(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["Alice", "Bob", "Charlie"])),
            ],
        )
        .expect("Failed to create test batch")
    }

    #[test]
    fn test_format_from_csv_extension() {
        assert_eq!(ExportFormat::from_path("output.csv").unwrap(), ExportFormat::Csv);
        assert_eq!(ExportFormat::from_path("output.CSV").unwrap(), ExportFormat::Csv);
        assert_eq!(ExportFormat::from_path("/tmp/data.csv").unwrap(), ExportFormat::Csv);
    }

    #[test]
    fn test_format_from_jsonl_extension() {
        assert_eq!(ExportFormat::from_path("output.jsonl").unwrap(), ExportFormat::JsonLines);
        assert_eq!(ExportFormat::from_path("output.JSONL").unwrap(), ExportFormat::JsonLines);
    }

    #[test]
    fn test_format_unsupported_extension() {
        let err = ExportFormat::from_path("output.xml").unwrap_err();
        assert!(err.to_string().contains("Unsupported export format"));
    }

    #[test]
    fn test_format_no_extension() {
        let err = ExportFormat::from_path("output").unwrap_err();
        assert!(err.to_string().contains("no extension"));
    }

    #[test]
    fn test_csv_export_writer() {
        let dir = std::env::temp_dir().join("bundlebase_test_csv_export");
        let path = dir.join("test.csv");
        std::fs::create_dir_all(&dir).unwrap();

        let schema = test_schema();
        let mut writer = create_export_writer(path.to_str().unwrap(), &schema).unwrap();
        writer.write_batch(&test_batch()).unwrap();
        let row_count = writer.finish().unwrap();

        assert_eq!(row_count, 3);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("id,name"));
        assert!(content.contains("Alice"));
        assert!(content.contains("Bob"));
        assert!(content.contains("Charlie"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_jsonl_export_writer() {
        let dir = std::env::temp_dir().join("bundlebase_test_jsonl_export");
        let path = dir.join("test.jsonl");
        std::fs::create_dir_all(&dir).unwrap();

        let schema = test_schema();
        let mut writer = create_export_writer(path.to_str().unwrap(), &schema).unwrap();
        writer.write_batch(&test_batch()).unwrap();
        let row_count = writer.finish().unwrap();

        assert_eq!(row_count, 3);
        let content = std::fs::read_to_string(&path).unwrap();
        // Each line should be a JSON object
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("Alice"));
        assert!(lines[1].contains("Bob"));
        assert!(lines[2].contains("Charlie"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_export_writer_multiple_batches() {
        let dir = std::env::temp_dir().join("bundlebase_test_multi_batch");
        let path = dir.join("test.csv");
        std::fs::create_dir_all(&dir).unwrap();

        let schema = test_schema();
        let mut writer = create_export_writer(path.to_str().unwrap(), &schema).unwrap();
        writer.write_batch(&test_batch()).unwrap();
        writer.write_batch(&test_batch()).unwrap();
        let row_count = writer.finish().unwrap();

        assert_eq!(row_count, 6);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
