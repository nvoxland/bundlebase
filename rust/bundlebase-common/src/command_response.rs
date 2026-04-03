//! Self-describing response types with Arrow schema support.
//!
//! This module provides the `CommandResponse` trait that all command outputs must implement,
//! enabling consistent handling of command results across different interfaces (REPL, Flight, etc.).

use arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
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
pub trait CommandResponse: Send {
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

    /// Convert this boxed response into a `SendableRecordBatchStream`.
    ///
    /// This is the sole data-producing method on the trait. Batch-based responses
    /// build a `RecordBatch` and wrap it via [`single_batch_stream`]. Stream-based
    /// responses (like `SendableRecordBatchStream`) return the stream directly.
    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Object-safe method to get schema at runtime via dynamic dispatch.
    ///
    /// This allows getting the schema from a `Box<dyn CommandResponse>` or `&dyn CommandResponse`.
    fn dyn_schema(&self) -> SchemaRef;

    /// Object-safe method to get output shape at runtime via dynamic dispatch.
    ///
    /// This allows getting the output shape from a `Box<dyn CommandResponse>` or `&dyn CommandResponse`.
    fn dyn_output_shape(&self) -> OutputShape;
}

/// Wrap a single `RecordBatch` into a `SendableRecordBatchStream`.
///
/// This is the standard helper for batch-based `CommandResponse` implementations.
pub fn single_batch_stream(
    schema: SchemaRef,
    batch: RecordBatch,
) -> Result<SendableRecordBatchStream, BundlebaseError> {
    let stream = futures::stream::iter(vec![Ok(batch)]);
    Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
}

/// Macro to implement the boilerplate `dyn_schema` and `dyn_output_shape` methods
/// that just delegate to `Self::schema()` and `Self::output_shape()`.
#[macro_export]
macro_rules! impl_dyn_command_response {
    ($ty:ty) => {
        fn dyn_schema(&self) -> ::arrow::datatypes::SchemaRef {
            <$ty as $crate::command_response::CommandResponse>::schema()
        }

        fn dyn_output_shape(&self) -> $crate::command_response::OutputShape {
            <$ty as $crate::command_response::CommandResponse>::output_shape()
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

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let message_array: ArrayRef = Arc::new(StringArray::from(vec![self.as_str()]));
        let batch = RecordBatch::try_new(Self::schema(), vec![message_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
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

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
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

        let batch =
            RecordBatch::try_new(Self::schema(), vec![columns_array, types_array, nullables_array])
                .map_err(|e| {
                    BundlebaseError::from(format!("Failed to create record batch: {}", e))
                })?;
        single_batch_stream(Self::schema(), batch)
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

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let count_array: ArrayRef = Arc::new(Int64Array::from(vec![*self as i64]));
        let batch = RecordBatch::try_new(Self::schema(), vec![count_array])
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
    }

    impl_dyn_command_response!(usize);
}

/// Implement CommandResponse for Vec<FetchResults> to allow fetch result outputs.
impl CommandResponse for Vec<crate::connector::FetchResults> {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("connector", DataType::Utf8, false),
            Field::new("source_url", DataType::Utf8, false),
            Field::new("pack", DataType::Utf8, false),
            Field::new("added_count", DataType::UInt64, false),
            Field::new("replaced_count", DataType::UInt64, false),
            Field::new("removed_count", DataType::UInt64, false),
            Field::new("rows_before", DataType::UInt64, false),
            Field::new("rows_after", DataType::UInt64, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let connector: ArrayRef = Arc::new(StringArray::from(
            self.iter()
                .map(|r| r.connector.as_str())
                .collect::<Vec<_>>(),
        ));
        let source_url: ArrayRef = Arc::new(StringArray::from(
            self.iter()
                .map(|r| r.source_url.as_str())
                .collect::<Vec<_>>(),
        ));
        let pack: ArrayRef = Arc::new(StringArray::from(
            self.iter().map(|r| r.pack.as_str()).collect::<Vec<_>>(),
        ));
        let added_count: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.added.len() as u64).collect::<Vec<_>>(),
        ));
        let replaced_count: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.replaced.len() as u64).collect::<Vec<_>>(),
        ));
        let removed_count: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.removed.len() as u64).collect::<Vec<_>>(),
        ));
        let rows_before: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.rows_before).collect::<Vec<_>>(),
        ));
        let rows_after: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.rows_after).collect::<Vec<_>>(),
        ));

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                connector,
                source_url,
                pack,
                added_count,
                replaced_count,
                removed_count,
                rows_before,
                rows_after,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
    }

    impl_dyn_command_response!(Vec<crate::connector::FetchResults>);
}

/// Implement CommandResponse for SendableRecordBatchStream so that
/// explain (and other stream-producing commands) can flow through the
/// normal command execution path without special-casing.
impl CommandResponse for SendableRecordBatchStream {
    fn schema() -> SchemaRef {
        // Not meaningful for streams — callers should use dyn_schema()
        Arc::new(Schema::empty())
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn dyn_schema(&self) -> SchemaRef {
        use datafusion::physical_plan::RecordBatchStream;
        RecordBatchStream::schema(self.as_ref().get_ref())
    }

    fn dyn_output_shape(&self) -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Ok(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_string_response() {
        let response: Box<dyn CommandResponse> = Box::new("Test message".to_string());
        let schema = response.dyn_schema();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "message");

        let mut stream = response.into_stream().expect("Failed to create stream");
        let batch = stream.next().await.unwrap().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }

    #[tokio::test]
    async fn test_usize_response() {
        let response: Box<dyn CommandResponse> = Box::new(42_usize);
        let schema = response.dyn_schema();
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "count");

        let mut stream = response.into_stream().expect("Failed to create stream");
        let batch = stream.next().await.unwrap().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }
}
