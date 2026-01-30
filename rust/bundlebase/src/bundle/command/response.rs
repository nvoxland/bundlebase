//! Self-describing response types with Arrow schema support.
//!
//! This module provides the `CommandResponse` trait that all command outputs must implement,
//! enabling consistent handling of command results across different interfaces (REPL, Flight, etc.).

use arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::BundlebaseError;

/// Describes the expected shape of command output for display formatting.
///
/// This enum helps REPL and other interfaces choose appropriate formatting:
/// - `SingleValue`: Display as plain text (e.g., "OK", a count, an explain plan)
/// - `Dictionary`: Display as key-value pairs (1 row, multiple columns)
/// - `Table`: Display as a formatted table (multiple rows expected)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputShape {
    /// Single value (1 row, 1 column) - display as plain text
    SingleValue,
    /// Key-value pairs (1 row, multiple columns) - display as dictionary
    Dictionary,
    /// Tabular data (multiple rows) - display as table
    Table,
}

/// Trait for command outputs that can describe their schema and convert to Arrow.
///
/// All command output types must implement this trait, enabling consistent handling
/// of results across different interfaces (REPL, Flight, Python bindings, etc.).
pub trait CommandResponse: Send + Sync {
    /// Returns the Arrow schema for this output type.
    ///
    /// This is an associated function that doesn't require an instance,
    /// allowing code to get the schema without having a value of this type.
    fn schema() -> SchemaRef
    where
        Self: Sized;

    /// Returns the expected output shape for display formatting.
    ///
    /// This helps interfaces choose appropriate formatting:
    /// - Vec types → Table (multiple rows expected)
    /// - Single column schema → SingleValue
    /// - Multi-column schema → Dictionary
    fn output_shape() -> OutputShape
    where
        Self: Sized;

    /// Converts this output to a RecordBatch.
    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError>;

    /// Object-safe method to get schema at runtime via dynamic dispatch.
    ///
    /// This allows getting the schema from a `Box<dyn CommandResponse>` or `&dyn CommandResponse`.
    fn dyn_schema(&self) -> SchemaRef;

    /// Object-safe method to get output shape at runtime via dynamic dispatch.
    ///
    /// This allows getting the output shape from a `Box<dyn CommandResponse>` or `&dyn CommandResponse`.
    fn dyn_output_shape(&self) -> OutputShape;
}

/// Macro to implement the boilerplate `dyn_schema` and `dyn_output_shape` methods
/// that just delegate to `Self::schema()` and `Self::output_shape()`.
#[macro_export]
macro_rules! impl_dyn_command_response {
    ($ty:ty) => {
        fn dyn_schema(&self) -> ::arrow::datatypes::SchemaRef {
            <$ty as $crate::bundle::command::CommandResponse>::schema()
        }

        fn dyn_output_shape(&self) -> $crate::bundle::command::response::OutputShape {
            <$ty as $crate::bundle::command::CommandResponse>::output_shape()
        }
    };
}

/// Implement CommandResponse for String to allow simple message outputs.
impl CommandResponse for String {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    fn output_shape() -> OutputShape {
        // String messages are single values (1 row, 1 column)
        OutputShape::SingleValue
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let message_array: ArrayRef = Arc::new(StringArray::from(vec![self.as_str()]));
        RecordBatch::try_new(Self::schema(), vec![message_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }

    impl_dyn_command_response!(String);
}

impl CommandResponse for SchemaRef {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("Column", DataType::Utf8, false),
            Field::new("Type", DataType::Utf8, false),
            Field::new("Nullable", DataType::Utf8, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let columns: Vec<&str> = self.fields().iter().map(|f| f.name().as_str()).collect();
        let types: Vec<String> = self.fields().iter().map(|f| f.data_type().to_string()).collect();
        let nullables: Vec<&str> = self
            .fields()
            .iter()
            .map(|f| if f.is_nullable() { "Yes" } else { "No" })
            .collect();

        let columns_array: ArrayRef = Arc::new(StringArray::from(columns));
        let types_array: ArrayRef = Arc::new(StringArray::from(types));
        let nullables_array: ArrayRef = Arc::new(StringArray::from(nullables));

        RecordBatch::try_new(Self::schema(), vec![columns_array, types_array, nullables_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }

    impl_dyn_command_response!(SchemaRef);
}

/// Implement CommandResponse for usize to allow count outputs.
impl CommandResponse for usize {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new("count", DataType::Int64, false)]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let count_array: ArrayRef = Arc::new(Int64Array::from(vec![*self as i64]));
        RecordBatch::try_new(Self::schema(), vec![count_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }

    impl_dyn_command_response!(usize);
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

    #[test]
    fn test_usize_schema() {
        let schema = usize::schema();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "count");
    }

    #[test]
    fn test_usize_to_record_batch() {
        let count: usize = 42;
        let batch = count.to_record_batch().expect("Failed to create batch");
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }
}
