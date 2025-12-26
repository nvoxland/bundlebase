use crate::data::RowId;
use crate::BundlebaseError;
use arrow::record_batch::RecordBatch;
use futures::stream::Stream;
use std::pin::Pin;

/// Type alias for a stream of RowIdBatches
pub type SendableRowIdBatchStream =
    Pin<Box<dyn Stream<Item = Result<RowIdBatch, BundlebaseError>> + Send>>;

/// Helper function to create a SendableRowIdBatchStream from a stream
pub fn boxed_rowid_stream<S>(stream: S) -> SendableRowIdBatchStream
where
    S: Stream<Item = Result<RowIdBatch, BundlebaseError>> + Send + 'static,
{
    Box::pin(stream)
}

/// A record batch paired with RowIds for index building
/// Used by extract_rowids_stream() to pass both data and row position info
#[derive(Debug)]
pub struct RowIdBatch {
    /// The actual data
    pub batch: RecordBatch,
    /// RowId for each row in the batch, in order
    /// row_ids[i] corresponds to batch.row(i)
    pub row_ids: Vec<RowId>,
}

impl RowIdBatch {
    pub fn new(batch: RecordBatch, row_ids: Vec<RowId>) -> Self {
        assert_eq!(
            batch.num_rows(),
            row_ids.len(),
            "Number of rows must match number of row IDs"
        );
        Self { batch, row_ids }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rowid_batch_creation() {
        use arrow::array::Int32Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let array = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(schema, vec![array]).unwrap();

        let row_ids = vec![RowId::from(0u64), RowId::from(1u64), RowId::from(2u64)];

        let rowid_batch = RowIdBatch::new(batch.clone(), row_ids.clone());

        assert_eq!(rowid_batch.batch.num_rows(), 3);
        assert_eq!(rowid_batch.row_ids.len(), 3);
        assert_eq!(rowid_batch.row_ids[0], RowId::from(0u64));
    }

    #[test]
    #[should_panic(expected = "Number of rows must match")]
    fn test_rowid_batch_mismatch() {
        use arrow::array::Int32Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let array = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(schema, vec![array]).unwrap();

        let row_ids = vec![RowId::from(0u64), RowId::from(1u64)]; // Only 2, but batch has 3

        RowIdBatch::new(batch, row_ids);
    }
}
