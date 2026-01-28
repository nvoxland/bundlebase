//! Self-describing response types with Arrow schema support.
//!
//! This module provides the `ToRecordBatch` trait that all command outputs must implement,
//! enabling consistent handling of command results across different interfaces (REPL, Flight, etc.).

use arrow::array::{ArrayRef, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::BundlebaseError;

/// Trait for command outputs that can describe their schema and convert to Arrow.
///
/// All command output types must implement this trait, enabling consistent handling
/// of results across different interfaces (REPL, Flight, Python bindings, etc.).
pub trait ToRecordBatch: Send + Sync {
    /// Returns the Arrow schema for this output type.
    ///
    /// This is an associated function that doesn't require an instance,
    /// allowing code to get the schema without having a value of this type.
    fn schema() -> SchemaRef
    where
        Self: Sized;

    /// Converts this output to a RecordBatch.
    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError>;
}

/// Implement ToRecordBatch for String to allow simple message outputs.
impl ToRecordBatch for String {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let message_array: ArrayRef = Arc::new(StringArray::from(vec![self.as_str()]));
        RecordBatch::try_new(Self::schema(), vec![message_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_schema() {
        let schema = String::schema();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "message");
    }

    #[test]
    fn test_string_to_record_batch() {
        let response = "Test message".to_string();
        let batch = response.to_record_batch().expect("Failed to create batch");
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }
}
