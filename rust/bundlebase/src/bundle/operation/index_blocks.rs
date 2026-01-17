use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::data::{ObjectId, RowId, VersionedBlockId};
use crate::index::{ColumnIndex, IndexedValue};
use crate::progress::ProgressScope;
use crate::{Bundle, BundlebaseError};
use arrow_schema::DataType;
use async_trait::async_trait;
use bytes::Bytes;
use datafusion::error::DataFusionError;
use datafusion::scalar::ScalarValue;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IndexBlocksOp {
    pub index: ObjectId,
    pub blocks: Vec<VersionedBlockId>,
    pub path: String,
    pub cardinality: u64,
}

/// Finds a block by ID in the bundle's packs.
fn find_block(bundle: &Bundle, block_id: &ObjectId) -> Result<Arc<DataBlock>, BundlebaseError> {
    for pack in bundle.packs().read().values() {
        for block in &pack.blocks() {
            if block.id() == block_id {
                return Ok(block.clone());
            }
        }
    }
    Err(BundlebaseError::from(format!(
        "Block {} not found in bundle",
        block_id
    )))
}

impl IndexBlocksOp {
    /// Builds and registers a column index across multiple blocks.
    ///
    /// Streams through all provided blocks for the specified column, accumulates value-to-rowid
    /// mappings, and creates a single ColumnIndex spanning all blocks. The index is then
    /// registered with the IndexManager and saved to disk.
    ///
    /// # Arguments
    /// * `index` - Unique identifier for this index operation
    /// * `column` - Column name to build index for
    /// * `blocks` - Vec of (block_id, version) tuples to index
    /// * `bundle` - Bundle providing block access and index management
    ///
    /// # Returns
    /// * `Ok(Self)` - Successfully created and registered index
    /// * `Err(e)` - Failed at any step (missing block, column, data type mismatch, etc.)
    ///
    /// # Errors
    /// Returns error if:
    /// - `blocks` is empty (cannot create index with no data)
    /// - Any block is not found in packs
    /// - Column doesn't exist in a block
    /// - Data types differ between blocks for the same column
    /// - Streaming or index building fails
    pub async fn setup(
        index: &ObjectId,
        column: &str,
        blocks: Vec<(ObjectId, String)>,
        bundle: &Bundle,
    ) -> Result<Self, BundlebaseError> {
        // Validate blocks is non-empty early
        if blocks.is_empty() {
            return Err(BundlebaseError::from("Cannot create index with no blocks"));
        }

        let mut all_value_to_rowids: HashMap<IndexedValue, Vec<RowId>> = HashMap::new();
        let mut data_type: Option<DataType> = None;

        // Create progress scope for tracking
        let _progress = ProgressScope::new(
            &format!("Indexing column '{}'", column),
            Some(blocks.len() as u64),
        );

        // For each block, stream data and build index mappings
        for (idx, (block_id, _version)) in blocks.iter().enumerate() {
            // Get the block from packs
            let block = find_block(bundle, block_id).map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to find block {} for indexing: {}",
                    block_id, e
                ))
            })?;

            // Get schema to find column index and data type
            let schema = block.schema();
            let (col_idx, field) = schema.column_with_name(column).ok_or_else(|| {
                BundlebaseError::from(format!(
                    "Column '{}' not found in block {}",
                    column, block_id,
                ))
            })?;

            // Validate data type consistency across blocks
            if let Some(ref existing_type) = data_type {
                if existing_type != field.data_type() {
                    return Err(BundlebaseError::from(format!(
                        "Data type mismatch for column '{}': {:?} in previous blocks vs {:?} in block {}",
                        column, existing_type, field.data_type(), block_id
                    )));
                }
            } else {
                data_type = Some(field.data_type().clone());
            }

            // Stream through block data
            let projection = Some(vec![col_idx]);
            let reader = block.reader();
            let mut rowid_stream = reader
                .extract_rowids_stream(bundle.ctx(), projection.as_ref())
                .await
                .map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to stream data from block {} for indexing: {}",
                        block_id, e
                    ))
                })?;

            while let Some(batch_result) = rowid_stream.next().await {
                let rowid_batch = batch_result.map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to read row batch from block {}: {}",
                        block_id, e
                    ))
                })?;
                let batch = rowid_batch.batch;
                let row_ids = rowid_batch.row_ids;

                let array = batch.column(0);

                // Build value -> rowid mapping
                for (row, row_id) in row_ids.iter().enumerate() {
                    let scalar = ScalarValue::try_from_array(array, row)?;
                    let indexed_value = IndexedValue::from_scalar(&scalar)?;

                    all_value_to_rowids
                        .entry(indexed_value)
                        .or_default()
                        .push(*row_id);
                }
            }

            // Update progress after each block
            let msg = format!("Block {}/{}", idx + 1, blocks.len());
            _progress.update((idx + 1) as u64, Some(&msg));
        }

        // Build the combined index
        // Now guaranteed to have data_type if we reach here (blocks is non-empty)
        let data_type_ref = data_type.as_ref().ok_or("No data type found for column")?;

        let column_index =
            ColumnIndex::build(column, data_type_ref, all_value_to_rowids).map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to build index for column '{}': {}",
                    column, e
                ))
            })?;

        let total_cardinality = column_index.cardinality();

        // Serialize the index and write using content-addressed storage
        let serialized = column_index.serialize().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to serialize index for column '{}': {}",
                column, e
            ))
        })?;

        // Create a stream from the serialized bytes
        let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(serialized) });
        let boxed_stream: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>> =
            Box::pin(stream);

        // Write using SHA-based naming with .index.idx extension
        let write_result = bundle
            .data_dir
            .write_stream(boxed_stream, "index.idx")
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to save index for column '{}': {}",
                    column, e
                ))
            })?;
        let written_file = write_result.file;

        // Get the relative path within the data_dir
        let rel_path = bundle.data_dir.relative_path(written_file.as_ref())?;

        log::debug!(
            "Successfully created index for column '{}' at {}",
            column,
            written_file.url()
        );

        Ok(Self {
            index: *index,
            blocks: blocks
                .into_iter()
                .map(|(block, version)| VersionedBlockId { block, version })
                .collect(),
            path: rel_path,
            cardinality: total_cardinality,
        })
    }
}

#[async_trait]
impl Operation for IndexBlocksOp {
    fn describe(&self) -> String {
        "INDEX BLOCKS".to_string()
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify all referenced blocks still exist in the bundle
        // This is a lightweight validation that doesn't require schema analysis
        for block_and_version in &self.blocks {
            find_block(bundle, &block_and_version.block).map_err(|_| {
                BundlebaseError::from(format!(
                    "Block {} referenced in index {} not found in bundle",
                    block_and_version, self.index
                ))
            })?;
        }

        // Note: Column existence and schema validation is performed during setup() when the
        // index is first created. We don't re-validate here to avoid expensive schema analysis
        // and because the index structure itself validates data types during build.
        Ok(())
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        // Find the corresponding IndexDefinition by index
        let index_def = {
            let indexes = bundle.indexes.read();
            indexes
                .iter()
                .find(|idx| idx.id() == &self.index)
                .cloned()
        };

        if let Some(index_def) = index_def {
            // Create IndexedBlocks instance with VersionedBlockId
            let indexed_blocks = Arc::new(crate::bundle::IndexedBlocks::new(
                self.blocks.clone(),
                self.path.clone(),
            ));

            // Add to the IndexDefinition
            index_def.add_indexed_blocks(indexed_blocks);

            log::debug!(
                "Added indexed blocks to index {} (column '{}'): {} blocks",
                self.index,
                index_def.column(),
                self.blocks.len()
            );

            Ok(())
        } else {
            Err(DataFusionError::Internal(format!(
                "IndexDefinition {} not found when applying IndexBlocksOp. \
                 The index may have been dropped or the manifest may be corrupted.",
                self.index
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_blocks_op_serialization() {
        let op = IndexBlocksOp {
            index: ObjectId::from(1),
            blocks: vec![
                VersionedBlockId::new(ObjectId::from(10), "v1".to_string()),
                VersionedBlockId::new(ObjectId::from(20), "v2".to_string()),
            ],
            path: "ab/cdef0123456789.index.idx".to_string(),
            cardinality: 100,
        };

        let json = serde_json::to_string(&op).unwrap();
        let deserialized: IndexBlocksOp = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized, op);
        assert_eq!(deserialized.blocks.len(), 2);
        assert_eq!(format!("{}", deserialized.blocks[0]), "0a@v1");
        assert_eq!(format!("{}", deserialized.blocks[1]), "14@v2");
    }
}
