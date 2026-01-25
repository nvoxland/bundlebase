//! Self-describing response types with Arrow schema support.
//!
//! This module provides response types that can convert themselves to Arrow RecordBatches,
//! enabling consistent handling of command results across different interfaces (REPL, Flight, etc.).

use arrow::array::{ArrayRef, BooleanArray, RecordBatch, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::bundle::{FileVerificationResult, VerificationResults};
use crate::source::FetchResults;
use crate::BundlebaseError;

/// Trait for command responses that can describe their schema and convert to Arrow.
pub trait CommandResponse: Send + Sync {
    /// Returns the Arrow schema for this response type.
    fn schema(&self) -> SchemaRef;

    /// Converts this response to a RecordBatch.
    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError>;
}

/// Simple "OK" message response for commands that complete without returning data.
#[derive(Debug, Clone)]
pub struct MessageResponse {
    pub message: String,
}

impl MessageResponse {
    /// Create a new message response.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Create a standard "OK" response.
    pub fn ok() -> Self {
        Self::new("OK")
    }
}

impl CommandResponse for MessageResponse {
    fn schema(&self) -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let message_array: ArrayRef = Arc::new(StringArray::from(vec![self.message.clone()]));
        RecordBatch::try_new(self.schema(), vec![message_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }
}

/// Row representation for VERIFY DATA results.
#[derive(Debug, Clone)]
pub struct VerificationRow {
    pub location: String,
    pub file_type: String,
    pub expected_hash: Option<String>,
    pub actual_hash: Option<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub version_updated: bool,
}

impl From<&FileVerificationResult> for VerificationRow {
    fn from(result: &FileVerificationResult) -> Self {
        Self {
            location: result.location.clone(),
            file_type: result.file_type.clone(),
            expected_hash: result.expected_hash.clone(),
            actual_hash: result.actual_hash.clone(),
            passed: result.passed,
            error: result.error.clone(),
            version_updated: result.version_updated,
        }
    }
}

/// Convert VerificationResults to a vector of VerificationRows.
pub fn verification_results_to_rows(results: &VerificationResults) -> Vec<VerificationRow> {
    results.files.iter().map(VerificationRow::from).collect()
}

/// Get the Arrow schema for verification results.
pub fn verification_schema() -> SchemaRef {
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

/// Convert VerificationResults to a RecordBatch.
pub fn verification_to_record_batch(
    results: &VerificationResults,
) -> Result<RecordBatch, BundlebaseError> {
    let rows = verification_results_to_rows(results);

    let location: ArrayRef = Arc::new(StringArray::from(
        rows.iter().map(|r| r.location.as_str()).collect::<Vec<_>>(),
    ));
    let file_type: ArrayRef = Arc::new(StringArray::from(
        rows.iter().map(|r| r.file_type.as_str()).collect::<Vec<_>>(),
    ));
    let expected_hash: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.expected_hash.as_deref())
            .collect::<Vec<_>>(),
    ));
    let actual_hash: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.actual_hash.as_deref())
            .collect::<Vec<_>>(),
    ));
    let passed: ArrayRef = Arc::new(BooleanArray::from(
        rows.iter().map(|r| r.passed).collect::<Vec<_>>(),
    ));
    let error: ArrayRef = Arc::new(StringArray::from(
        rows.iter().map(|r| r.error.as_deref()).collect::<Vec<_>>(),
    ));
    let version_updated: ArrayRef = Arc::new(BooleanArray::from(
        rows.iter().map(|r| r.version_updated).collect::<Vec<_>>(),
    ));

    RecordBatch::try_new(
        verification_schema(),
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
    .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
}

/// Row representation for FETCH results.
#[derive(Debug, Clone)]
pub struct FetchRow {
    pub source_function: String,
    pub source_url: String,
    pub pack: String,
    pub added_count: u64,
    pub replaced_count: u64,
    pub removed_count: u64,
}

impl From<&FetchResults> for FetchRow {
    fn from(result: &FetchResults) -> Self {
        Self {
            source_function: result.source_function.clone(),
            source_url: result.source_url.clone(),
            pack: result.pack.clone(),
            added_count: result.added.len() as u64,
            replaced_count: result.replaced.len() as u64,
            removed_count: result.removed.len() as u64,
        }
    }
}

/// Convert Vec<FetchResults> to a vector of FetchRows.
pub fn fetch_results_to_rows(results: &[FetchResults]) -> Vec<FetchRow> {
    results.iter().map(FetchRow::from).collect()
}

/// Get the Arrow schema for fetch results.
pub fn fetch_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("source_function", DataType::Utf8, false),
        Field::new("source_url", DataType::Utf8, false),
        Field::new("pack", DataType::Utf8, false),
        Field::new("added_count", DataType::UInt64, false),
        Field::new("replaced_count", DataType::UInt64, false),
        Field::new("removed_count", DataType::UInt64, false),
    ]))
}

/// Convert Vec<FetchResults> to a RecordBatch.
pub fn fetch_to_record_batch(results: &[FetchResults]) -> Result<RecordBatch, BundlebaseError> {
    let rows = fetch_results_to_rows(results);

    let source_function: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.source_function.as_str())
            .collect::<Vec<_>>(),
    ));
    let source_url: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|r| r.source_url.as_str())
            .collect::<Vec<_>>(),
    ));
    let pack: ArrayRef = Arc::new(StringArray::from(
        rows.iter().map(|r| r.pack.as_str()).collect::<Vec<_>>(),
    ));
    let added_count: ArrayRef = Arc::new(UInt64Array::from(
        rows.iter().map(|r| r.added_count).collect::<Vec<_>>(),
    ));
    let replaced_count: ArrayRef = Arc::new(UInt64Array::from(
        rows.iter().map(|r| r.replaced_count).collect::<Vec<_>>(),
    ));
    let removed_count: ArrayRef = Arc::new(UInt64Array::from(
        rows.iter().map(|r| r.removed_count).collect::<Vec<_>>(),
    ));

    RecordBatch::try_new(
        fetch_schema(),
        vec![
            source_function,
            source_url,
            pack,
            added_count,
            replaced_count,
            removed_count,
        ],
    )
    .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
}

/// Row representation for EXPLAIN PLAN results.
#[derive(Debug, Clone)]
pub struct PlanRow {
    pub plan: String,
}

/// Get the Arrow schema for plan results.
pub fn plan_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("plan", DataType::Utf8, false)]))
}

/// Convert a plan string to a RecordBatch.
pub fn plan_to_record_batch(plan: &str) -> Result<RecordBatch, BundlebaseError> {
    let plan_array: ArrayRef = Arc::new(StringArray::from(vec![plan]));
    RecordBatch::try_new(plan_schema(), vec![plan_array])
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
}

/// Get the Arrow schema for message results.
pub fn message_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        "message",
        DataType::Utf8,
        false,
    )]))
}

/// Convert a message to a RecordBatch.
pub fn message_to_record_batch(message: &str) -> Result<RecordBatch, BundlebaseError> {
    let message_array: ArrayRef = Arc::new(StringArray::from(vec![message]));
    RecordBatch::try_new(message_schema(), vec![message_array])
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_response_schema() {
        let response = MessageResponse::ok();
        let schema = response.schema();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "message");
    }

    #[test]
    fn test_message_response_to_record_batch() {
        let response = MessageResponse::new("Test message");
        let batch = response.to_record_batch().expect("Failed to create batch");
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }

    #[test]
    fn test_verification_schema() {
        let schema = verification_schema();
        assert_eq!(schema.fields().len(), 7);
        assert_eq!(schema.field(0).name(), "location");
        assert_eq!(schema.field(4).name(), "passed");
    }

    #[test]
    fn test_fetch_schema() {
        let schema = fetch_schema();
        assert_eq!(schema.fields().len(), 6);
        assert_eq!(schema.field(0).name(), "source_function");
        assert_eq!(schema.field(3).name(), "added_count");
    }

    #[test]
    fn test_plan_schema() {
        let schema = plan_schema();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "plan");
    }

    #[test]
    fn test_plan_to_record_batch() {
        let batch = plan_to_record_batch("Test plan").expect("Failed to create batch");
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }
}
