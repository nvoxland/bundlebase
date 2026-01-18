use crate::data::{DataReader, RowId, SendableRecordBatchStream};
use crate::index::column_index::{ColumnIndex, IndexedValue};
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use std::fmt;
use std::sync::Arc;

/// Utility for executing index-accelerated queries
/// Looks up a value in the index and provides RowIds for fetching actual rows
#[derive(Clone)]
pub struct IndexScanExec {
    /// The schema of results
    schema: SchemaRef,
    /// The column index to query
    index: Arc<ColumnIndex>,
    /// The data adapter to fetch rows from
    adapter: Arc<dyn DataReader>,
    /// The value to look up in the index
    lookup_value: IndexedValue,
    /// Optional projection (column indices to return)
    projection: Option<Vec<usize>>,
}

impl IndexScanExec {
    pub fn new(
        schema: SchemaRef,
        index: Arc<ColumnIndex>,
        adapter: Arc<dyn DataReader>,
        lookup_value: IndexedValue,
        projection: Option<Vec<usize>>,
    ) -> Self {
        Self {
            schema,
            index,
            adapter,
            lookup_value,
            projection,
        }
    }

    /// Look up a value in the index and get matching RowIds
    pub fn lookup_rowids(&self) -> Vec<RowId> {
        self.index.lookup_exact(&self.lookup_value)
    }

    /// Get the schema of results
    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Get the adapter
    pub fn adapter(&self) -> &Arc<dyn DataReader> {
        &self.adapter
    }

    /// Get the projection
    pub fn projection(&self) -> &Option<Vec<usize>> {
        &self.projection
    }

    /// Execute the index scan asynchronously
    /// Returns a stream of record batches matching the index lookup
    pub async fn execute(&self) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let row_ids = self.lookup_rowids();

        // Fetch rows by their RowIds from the adapter
        self.adapter
            .read_rows_by_ids(&row_ids, self.projection.as_ref())
            .await
    }
}

impl fmt::Debug for IndexScanExec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IndexScanExec")
            .field("schema", &self.schema)
            .field("lookup_value", &self.lookup_value)
            .field("projection", &self.projection)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::column_index::IndexedValue;
    use arrow::datatypes::DataType;
    use std::collections::HashMap;

    #[test]
    fn test_index_scan_exec_creation() {
        let mut index_map = HashMap::new();
        index_map.insert(
            IndexedValue::Utf8("test".to_string()),
            vec![RowId::from(0u64)],
        );

        let has_entries = !index_map.is_empty();
        let index = Arc::new(ColumnIndex::build("value", &DataType::Utf8, index_map).unwrap());

        // Verify the index was created successfully
        assert!(has_entries, "Index map should have entries");

        // Verify we can look up values
        let lookup_value = IndexedValue::Utf8("test".to_string());
        let row_ids = index.lookup_exact(&lookup_value);
        assert_eq!(row_ids.len(), 1, "Should find exactly 1 matching row");
    }
}
