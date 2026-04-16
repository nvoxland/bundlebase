//! Verification result types for bundle data integrity checks.

use crate::BundlebaseError;
use arrow::array::{ArrayRef, BooleanArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use bundlebase_common::command_response::{single_batch_stream, CommandResponse, OutputShape};
use bundlebase_common::impl_dyn_command_response;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Result of verifying a single file
#[derive(Debug, Clone)]
pub struct FileVerificationResult {
    pub location: String,
    pub file_type: String, // "data" or "index"
    pub expected_hash: Option<String>,
    pub actual_hash: Option<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub version_updated: bool,
}

/// Complete verification results for a bundle
#[derive(Debug, Clone)]
pub struct VerificationResults {
    pub files: Vec<FileVerificationResult>,
    pub passed_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    pub versions_updated_count: usize,
    pub all_passed: bool,
}

impl VerificationResults {
    /// Create a new `VerificationResults` from a list of file verification results.
    pub fn from_files(files: Vec<FileVerificationResult>) -> Self {
        let passed_count = files.iter().filter(|f| f.passed).count();
        let failed_count = files.iter().filter(|f| !f.passed).count();
        let skipped_count = files
            .iter()
            .filter(|f| f.passed && f.expected_hash.is_none())
            .count();
        let versions_updated_count = files.iter().filter(|f| f.version_updated).count();
        let all_passed = failed_count == 0;

        VerificationResults {
            files,
            passed_count,
            failed_count,
            skipped_count,
            versions_updated_count,
            all_passed,
        }
    }

    /// Check verification results and return error if any files failed.
    pub fn check(&self) -> Result<(), BundlebaseError> {
        let failures: Vec<&FileVerificationResult> =
            self.files.iter().filter(|f| !f.passed).collect();

        if failures.is_empty() {
            return Ok(());
        }

        let messages: Vec<String> = failures
            .iter()
            .map(|f| {
                if let Some(ref err) = f.error {
                    format!("{}: {}", f.location, err)
                } else if f.expected_hash != f.actual_hash {
                    format!(
                        "{}: hash mismatch (expected {}, got {})",
                        f.location,
                        f.expected_hash.as_deref().unwrap_or("none"),
                        f.actual_hash.as_deref().unwrap_or("none")
                    )
                } else {
                    format!("{}: verification failed", f.location)
                }
            })
            .collect();

        Err(BundlebaseError::from(format!(
            "Data verification failed for {} file(s):\n{}",
            failures.len(),
            messages.join("\n")
        )))
    }
}

impl CommandResponse for VerificationResults {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("location", DataType::Utf8, false),
            Field::new("file_type", DataType::Utf8, false),
            Field::new("expected_hash", DataType::Utf8, true),
            Field::new("actual_hash", DataType::Utf8, true),
            Field::new("passed", DataType::Boolean, false),
            Field::new("error", DataType::Utf8, true),
            Field::new("version_updated", DataType::Boolean, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let files = &self.files;

        let location: ArrayRef = Arc::new(StringArray::from(
            files
                .iter()
                .map(|r| r.location.as_str())
                .collect::<Vec<_>>(),
        ));
        let file_type: ArrayRef = Arc::new(StringArray::from(
            files
                .iter()
                .map(|r| r.file_type.as_str())
                .collect::<Vec<_>>(),
        ));
        let expected_hash: ArrayRef = Arc::new(StringArray::from(
            files
                .iter()
                .map(|r| r.expected_hash.as_deref())
                .collect::<Vec<_>>(),
        ));
        let actual_hash: ArrayRef = Arc::new(StringArray::from(
            files
                .iter()
                .map(|r| r.actual_hash.as_deref())
                .collect::<Vec<_>>(),
        ));
        let passed: ArrayRef = Arc::new(BooleanArray::from(
            files.iter().map(|r| r.passed).collect::<Vec<_>>(),
        ));
        let error: ArrayRef = Arc::new(StringArray::from(
            files.iter().map(|r| r.error.as_deref()).collect::<Vec<_>>(),
        ));
        let version_updated: ArrayRef = Arc::new(BooleanArray::from(
            files.iter().map(|r| r.version_updated).collect::<Vec<_>>(),
        ));

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                location,
                file_type,
                expected_hash,
                actual_hash,
                passed,
                error,
                version_updated,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
    }

    impl_dyn_command_response!(VerificationResults);
}

impl std::fmt::Display for VerificationResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.all_passed {
            write!(f, "All {} files verified successfully", self.passed_count)
        } else {
            writeln!(
                f,
                "Verification: {} passed, {} failed",
                self.passed_count, self.failed_count
            )?;
            for file in self.files.iter().filter(|file| !file.passed) {
                write!(f, "  FAILED: {}", file.location)?;
                if let Some(ref err) = file.error {
                    write!(f, " ({})", err)?;
                }
                writeln!(f)?;
            }
            Ok(())
        }
    }
}
