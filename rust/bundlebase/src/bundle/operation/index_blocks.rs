use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::data::{ObjectId, RowId, VersionedBlockId};
use crate::index::{
    ColumnIndex, ExternalSortConfig, ExternalSortWriter, IndexedValue, IndexType, TempDirManager,
    TextColumnIndex, TokenizerConfig, DEFAULT_MEMORY_LIMIT_BYTES,
};
use crate::progress::ProgressScope;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use arrow::record_batch::RecordBatch;
use arrow_schema::DataType;
use async_trait::async_trait;
use bytes::Bytes;
use datafusion::error::DataFusionError;
use datafusion::scalar::ScalarValue;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IndexBlocksOp {
    pub index: ObjectId,
    pub blocks: Vec<VersionedBlockId>,
    pub path: String,
    pub cardinality: u64,
    /// Document count for text indexes (number of rows indexed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_count: Option<u64>,
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

/// Information about a block for index building
struct BlockInfo {
    block: Arc<DataBlock>,
    col_idx: usize,
    data_type: DataType,
}

/// Validates and prepares blocks for index building.
///
/// This helper function extracts the common pattern of:
/// - Finding blocks in the bundle
/// - Validating column existence
/// - Optionally validating data type consistency or specific types
///
/// # Arguments
/// * `blocks` - Block IDs and versions to validate
/// * `bundle` - Bundle containing the blocks
/// * `column` - Column name to index
/// * `data_type_validator` - Optional validator for data type requirements
fn prepare_blocks_for_indexing<F>(
    blocks: &[(ObjectId, String)],
    bundle: &Bundle,
    column: &str,
    data_type_validator: F,
) -> Result<Vec<BlockInfo>, BundlebaseError>
where
    F: Fn(&DataType, &ObjectId) -> Result<(), BundlebaseError>,
{
    let mut block_infos = Vec::with_capacity(blocks.len());

    for (block_id, _version) in blocks.iter() {
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

        let data_type = field.data_type().clone();

        // Validate data type
        data_type_validator(&data_type, block_id)?;

        block_infos.push(BlockInfo {
            block,
            col_idx,
            data_type,
        });
    }

    Ok(block_infos)
}

/// Iterates through blocks and calls the processor for each batch.
///
/// This helper extracts the common streaming pattern used in both column and text index building.
///
/// # Arguments
/// * `block_infos` - Prepared block information
/// * `bundle` - Bundle for context
/// * `progress` - Progress scope for tracking
/// * `processor` - Callback for each (batch, row_ids) pair
async fn iterate_blocks<F>(
    block_infos: &[BlockInfo],
    bundle: &Bundle,
    progress: &ProgressScope,
    mut processor: F,
) -> Result<(), BundlebaseError>
where
    F: FnMut(&RecordBatch, &[RowId]) -> Result<(), BundlebaseError>,
{
    for (idx, block_info) in block_infos.iter().enumerate() {
        let projection = Some(vec![block_info.col_idx]);
        let reader = block_info.block.reader();
        let mut rowid_stream = reader
            .extract_rowids_stream(bundle.ctx(), projection.as_ref())
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to stream data from block for indexing: {}",
                    e
                ))
            })?;

        while let Some(batch_result) = rowid_stream.next().await {
            let rowid_batch = batch_result.map_err(|e| {
                BundlebaseError::from(format!("Failed to read row batch from block: {}", e))
            })?;

            processor(&rowid_batch.batch, &rowid_batch.row_ids)?;
        }

        // Update progress after each block
        let msg = format!("Block {}/{}", idx + 1, block_infos.len());
        progress.update((idx + 1) as u64, Some(&msg));
    }

    Ok(())
}

impl IndexBlocksOp {
    /// Builds and registers an index across multiple blocks.
    ///
    /// Streams through all provided blocks for the specified column, accumulates value-to-rowid
    /// mappings, and creates either a ColumnIndex or TextColumnIndex based on the index type.
    /// The index is then registered with the IndexManager and saved to disk.
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
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let bundle = builder.bundle();

        // Validate blocks is non-empty early
        if blocks.is_empty() {
            return Err(BundlebaseError::from("Cannot create index with no blocks"));
        }

        // Look up the index definition to get its type
        let index_type = {
            let indexes = bundle.indexes().read();
            indexes
                .iter()
                .find(|idx| idx.id() == index)
                .map(|idx| idx.index_type().clone())
                .ok_or_else(|| {
                    BundlebaseError::from(format!(
                        "Index definition {} not found. CreateIndexOp must be applied first.",
                        index
                    ))
                })?
        };

        // Dispatch to appropriate index building method
        match index_type {
            IndexType::Column => Self::build_column_index(index, column, blocks, bundle).await,
            IndexType::Text { tokenizer } => {
                Self::build_text_index(index, column, blocks, bundle, &tokenizer).await
            }
        }
    }

    /// Build a column index (B-tree style for equality/range queries)
    ///
    /// Uses streaming external sort to build indexes larger than available RAM.
    /// The process:
    /// 1. Stream through all blocks, adding (value, rowid) pairs to external sorter
    /// 2. External sorter flushes sorted runs to disk when memory limit exceeded
    /// 3. K-way merge produces sorted stream of entries
    /// 4. Build index incrementally from sorted stream
    async fn build_column_index(
        index: &ObjectId,
        column: &str,
        blocks: Vec<(ObjectId, String)>,
        bundle: &Bundle,
    ) -> Result<Self, BundlebaseError> {
        // Prepare blocks first to get all data types
        let block_infos = prepare_blocks_for_indexing(
            &blocks,
            bundle,
            column,
            |_data_type, _block_id| Ok(()), // Initial validation - just check column exists
        )?;

        // Validate data type consistency across all blocks
        if let Some(first_info) = block_infos.first() {
            let expected_type = &first_info.data_type;
            for (idx, block_info) in block_infos.iter().enumerate().skip(1) {
                if &block_info.data_type != expected_type {
                    return Err(BundlebaseError::from(format!(
                        "Data type mismatch for column '{}': {:?} in block 0 vs {:?} in block {}",
                        column, expected_type, block_info.data_type, idx
                    )));
                }
            }
        }

        // Get the data type from the first block (we know it's non-empty due to earlier validation)
        let data_type = block_infos
            .first()
            .map(|bi| bi.data_type.clone())
            .ok_or_else(|| BundlebaseError::from("No blocks to index"))?;

        // Create progress scope for tracking
        let progress = ProgressScope::new(
            &format!("Indexing column '{}'", column),
            Some(blocks.len() as u64),
        );

        // Create temp directory for external sort
        let temp_manager = TempDirManager::new(&bundle.data_dir(), "column_index")?;

        let sort_config = ExternalSortConfig::new(
            DEFAULT_MEMORY_LIMIT_BYTES,
            temp_manager.path().clone(),
        );
        let mut sorter = ExternalSortWriter::new(sort_config)?;

        // Stream entries to sorter (replaces HashMap accumulation)
        iterate_blocks(&block_infos, bundle, &progress, |batch, row_ids| {
            let array = batch.column(0);

            for (row, row_id) in row_ids.iter().enumerate() {
                let scalar = ScalarValue::try_from_array(array, row)?;
                let indexed_value = IndexedValue::from_scalar(&scalar)?;
                sorter.add(indexed_value, *row_id)?;
            }
            Ok(())
        })
        .await?;

        // Build index from sorted stream
        let sorted_iter = sorter.finish()?;
        let column_index = ColumnIndex::build_streaming(
            column,
            &data_type,
            sorted_iter.map(|r| r.map(|e| (e.value, e.row_id))),
        )
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to build index for column '{}': {}",
                column, e
            ))
        })?;

        let total_cardinality = column_index.cardinality();

        // Serialize and save the index
        let serialized = column_index.serialize().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to serialize index for column '{}': {}",
                column, e
            ))
        })?;

        let rel_path = Self::save_index_bytes(bundle, serialized, "index.idx", column).await?;

        log::debug!(
            "Successfully created column index for '{}' at {}",
            column,
            rel_path
        );

        Ok(Self {
            index: *index,
            blocks: blocks
                .into_iter()
                .map(|(block, version)| VersionedBlockId { block, version })
                .collect(),
            path: rel_path,
            cardinality: total_cardinality,
            doc_count: None,
        })
    }

    /// Save serialized index bytes to storage and return the relative path
    async fn save_index_bytes(
        bundle: &Bundle,
        serialized: Bytes,
        extension: &str,
        column: &str,
    ) -> Result<String, BundlebaseError> {
        let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(serialized) });
        let boxed_stream: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>> =
            Box::pin(stream);

        let data_dir = bundle.data_dir();
        let write_result = data_dir
            .write_stream(boxed_stream, extension)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to save index for column '{}': {}",
                    column, e
                ))
            })?;

        data_dir.relative_path(write_result.file.as_ref())
    }

    /// Build a text index (BM25 full-text search)
    ///
    /// Uses streaming to collect documents, then builds via Tantivy's streaming builder.
    /// Tantivy's internal 50MB heap handles batching during index construction.
    async fn build_text_index(
        index: &ObjectId,
        column: &str,
        blocks: Vec<(ObjectId, String)>,
        bundle: &Bundle,
        tokenizer_config: &TokenizerConfig,
    ) -> Result<Self, BundlebaseError> {
        // Validator that ensures the column is a string type
        let column_for_error = column.to_string();
        let string_type_validator = move |data_type: &DataType, block_id: &ObjectId| {
            match data_type {
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Ok(()),
                other => Err(BundlebaseError::from(format!(
                    "Text index requires string column, but '{}' in block {} has type {:?}",
                    column_for_error, block_id, other
                ))),
            }
        };

        // Prepare and validate blocks
        let block_infos = prepare_blocks_for_indexing(&blocks, bundle, column, string_type_validator)?;

        // Create progress scope for tracking
        let progress = ProgressScope::new(
            &format!("Building text index for '{}'", column),
            Some(blocks.len() as u64),
        );

        // Collect documents as (text, rowid) pairs for streaming build
        // Tantivy's 50MB heap handles batching during indexing
        let mut documents: Vec<(String, RowId)> = Vec::new();

        // Iterate through all blocks and process batches
        iterate_blocks(&block_infos, bundle, &progress, |batch, row_ids| {
            let array = batch.column(0);

            for (row, row_id) in row_ids.iter().enumerate() {
                let scalar = ScalarValue::try_from_array(array, row)?;

                // Extract string value, skipping nulls
                let text_value = match &scalar {
                    ScalarValue::Utf8(Some(s)) | ScalarValue::LargeUtf8(Some(s)) => s.clone(),
                    ScalarValue::Utf8View(Some(s)) => s.to_string(),
                    _ => continue, // Skip nulls
                };

                documents.push((text_value, *row_id));
            }
            Ok(())
        })
        .await?;

        // Build the text index using streaming builder
        let text_index = TextColumnIndex::build_streaming(
            column,
            documents.into_iter(),
            tokenizer_config,
        )
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to build text index for column '{}': {}",
                column, e
            ))
        })?;

        let doc_count = text_index.doc_count();

        // Serialize and save the index
        let serialized = text_index.serialize().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to serialize text index for column '{}': {}",
                column, e
            ))
        })?;

        let rel_path = Self::save_index_bytes(bundle, serialized, "textindex.tar", column).await?;

        log::debug!(
            "Successfully created text index for '{}' at {} ({} documents)",
            column,
            rel_path,
            doc_count
        );

        Ok(Self {
            index: *index,
            blocks: blocks
                .into_iter()
                .map(|(block, version)| VersionedBlockId { block, version })
                .collect(),
            path: rel_path,
            cardinality: doc_count, // For text indexes, cardinality = unique text values indexed
            doc_count: Some(doc_count),
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

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
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
            doc_count: None,
        };

        let json = serde_json::to_string(&op).expect("Serialization should succeed");
        let deserialized: IndexBlocksOp =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized, op);
        assert_eq!(deserialized.blocks.len(), 2);
        assert_eq!(format!("{}", deserialized.blocks[0]), "0a@v1");
        assert_eq!(format!("{}", deserialized.blocks[1]), "14@v2");
    }

    #[test]
    fn test_index_blocks_op_serialization_with_doc_count() {
        let op = IndexBlocksOp {
            index: ObjectId::from(1),
            blocks: vec![VersionedBlockId::new(
                ObjectId::from(10),
                "v1".to_string(),
            )],
            path: "ab/cdef0123456789.textindex.tar".to_string(),
            cardinality: 50,
            doc_count: Some(150),
        };

        let json = serde_json::to_string(&op).expect("Serialization should succeed");
        assert!(json.contains("\"docCount\":150"));

        let deserialized: IndexBlocksOp =
            serde_json::from_str(&json).expect("Deserialization should succeed");
        assert_eq!(deserialized.doc_count, Some(150));
    }
}
