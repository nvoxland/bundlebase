use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::data::{BlockId, ObjectId, ObjectIdAlias, RowId, VersionedBlockId};
use crate::index::{
    ColumnIndex, ExternalSortConfig, ExternalSortWriter, IndexedValue, IndexType, TempDirManager,
    TextIndexBuilder, TokenizerConfig, DEFAULT_MEMORY_LIMIT_BYTES,
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
fn find_block(bundle: &Bundle, block_id: &BlockId) -> Result<Arc<DataBlock>, BundlebaseError> {
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
    blocks: &[(BlockId, String)],
    bundle: &Bundle,
    column: &str,
    data_type_validator: F,
) -> Result<Vec<BlockInfo>, BundlebaseError>
where
    F: Fn(&DataType, &BlockId) -> Result<(), BundlebaseError>,
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
/// Each block is assigned a sequential ObjectIdAlias for compact RowId encoding.
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
        // Assign a sequential ObjectIdAlias to each block for compact RowId encoding
        let block_ref = ObjectIdAlias::from(idx as u16);
        let mut rowid_stream = reader
            .extract_rowids_stream(block_ref, bundle.ctx(), projection.as_ref())
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
    /// Streams through all provided blocks for the specified columns, accumulates value-to-rowid
    /// mappings, and creates either a ColumnIndex or TextIndex based on the index type.
    /// The index is then registered with the IndexManager and saved to disk.
    ///
    /// # Arguments
    /// * `index` - Unique identifier for this index operation
    /// * `columns` - Column names to build index for
    /// * `blocks` - Vec of (block_id, version) tuples to index
    /// * `builder` - BundleBuilder providing block access and index management
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
        columns: Vec<String>,
        blocks: Vec<(BlockId, String)>,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let bundle = builder.bundle();

        // Validate blocks is non-empty early
        if blocks.is_empty() {
            return Err(BundlebaseError::from("Cannot create index with no blocks"));
        }

        // Look up the index definition to get its type and name
        let (index_type, index_name) = {
            let indexes = bundle.indexes().read();
            let idx = indexes
                .iter()
                .find(|idx| idx.id() == index)
                .ok_or_else(|| {
                    BundlebaseError::from(format!(
                        "Index definition {} not found. CreateIndexOp must be applied first.",
                        index
                    ))
                })?;
            (idx.index_type().clone(), idx.name().to_string())
        };

        // Dispatch to appropriate index building method
        match &index_type {
            IndexType::Column => {
                let column = columns.first().ok_or_else(|| {
                    BundlebaseError::from("Column index requires at least one column")
                })?;
                Self::build_column_index(index, column, blocks, bundle).await
            }
            IndexType::Text { tokenizer, .. } => {
                Self::build_text_index(index, &index_name, &columns, blocks, bundle, tokenizer).await
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
        blocks: Vec<(BlockId, String)>,
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

        let rel_path = Self::save_index_bytes(bundle, serialized, "idx.column", column).await?;

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
    /// Indexes one or more text columns into a single searchable index.
    /// Uses streaming to collect documents, then builds via Tantivy's streaming builder.
    /// Tantivy's internal 50MB heap handles batching during index construction.
    async fn build_text_index(
        index: &ObjectId,
        index_name: &str,
        text_columns: &[String],
        blocks: Vec<(BlockId, String)>,
        bundle: &Bundle,
        tokenizer_config: &TokenizerConfig,
    ) -> Result<Self, BundlebaseError> {
        // Create progress scope for tracking
        let progress = ProgressScope::new(
            &format!("Building text index for '{}'", index_name),
            Some(blocks.len() as u64),
        );

        // Build the text index incrementally — documents are fed directly to Tantivy
        // via TextIndexBuilder instead of buffering the entire corpus in memory.
        let mut builder = TextIndexBuilder::new(index_name, text_columns, tokenizer_config)
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to create text index builder for '{}': {}",
                    index_name, e
                ))
            })?;
        let num_columns = text_columns.len();

        // For each block, project all text columns, extract values, and build documents
        for (block_idx, (block_id, _version)) in blocks.iter().enumerate() {
            let block = find_block(bundle, block_id)?;
            let schema = block.schema();

            // Find column indices for all text columns
            let mut col_indices = Vec::new();
            for col_name in text_columns {
                let (col_idx, field) = schema.column_with_name(col_name).ok_or_else(|| {
                    BundlebaseError::from(format!(
                        "Column '{}' not found in block {}",
                        col_name, block_id,
                    ))
                })?;

                // Validate string type
                match field.data_type() {
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {}
                    other => {
                        return Err(BundlebaseError::from(format!(
                            "Text index requires string column, but '{}' in block {} has type {:?}",
                            col_name, block_id, other
                        )));
                    }
                }

                col_indices.push(col_idx);
            }

            // Project all text columns
            let projection: Vec<usize> = col_indices.clone();
            let reader = block.reader();
            let block_ref = ObjectIdAlias::from(block_idx as u16);
            let mut rowid_stream = reader
                .extract_rowids_stream(block_ref, bundle.ctx(), Some(&projection))
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

                let batch = &rowid_batch.batch;
                let row_ids = &rowid_batch.row_ids;

                for (row, row_id) in row_ids.iter().enumerate() {
                    let mut column_values: Vec<Option<String>> = Vec::with_capacity(num_columns);
                    let mut has_any_value = false;

                    for proj_idx in 0..num_columns {
                        let array = batch.column(proj_idx);
                        let scalar = ScalarValue::try_from_array(array, row)?;

                        let text_value = match &scalar {
                            ScalarValue::Utf8(Some(s)) | ScalarValue::LargeUtf8(Some(s)) => {
                                Some(s.clone())
                            }
                            ScalarValue::Utf8View(Some(s)) => Some(s.to_string()),
                            _ => None, // Null for this column
                        };

                        if text_value.is_some() {
                            has_any_value = true;
                        }
                        column_values.push(text_value);
                    }

                    // Only add document if at least one column has a value
                    if has_any_value {
                        builder.add_document(&column_values, *row_id)?;
                    }
                }
            }

            // Update progress after each block
            let msg = format!("Block {}/{}", block_idx + 1, blocks.len());
            progress.update((block_idx + 1) as u64, Some(&msg));
        }

        // Finalize the text index
        let text_index = builder.finish().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to finalize text index for '{}': {}",
                index_name, e
            ))
        })?;

        let doc_count = text_index.doc_count();

        // Serialize and save the index
        let serialized = text_index.serialize().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to serialize text index for '{}': {}",
                index_name, e
            ))
        })?;

        let rel_path =
            Self::save_index_bytes(bundle, serialized, "idx.text.tar", index_name).await?;

        log::debug!(
            "Successfully created text index '{}' for columns [{}] at {} ({} documents)",
            index_name,
            text_columns.join(", "),
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
            cardinality: doc_count,
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
                index_def.columns().join(", "),
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
        let index_id = ObjectId::generate();
        let block_id1 = BlockId::generate();
        let block_id2 = BlockId::generate();
        let op = IndexBlocksOp {
            index: index_id,
            blocks: vec![
                VersionedBlockId::new(block_id1, "v1".to_string()),
                VersionedBlockId::new(block_id2, "v2".to_string()),
            ],
            path: "ab/cdef0123456789.idx.column".to_string(),
            cardinality: 100,
            doc_count: None,
        };

        let json = serde_json::to_string(&op).expect("Serialization should succeed");
        let deserialized: IndexBlocksOp =
            serde_json::from_str(&json).expect("Deserialization should succeed");

        assert_eq!(deserialized, op);
        assert_eq!(deserialized.blocks.len(), 2);
    }

    #[test]
    fn test_index_blocks_op_serialization_with_doc_count() {
        let index_id = ObjectId::generate();
        let block_id = BlockId::generate();
        let op = IndexBlocksOp {
            index: index_id,
            blocks: vec![VersionedBlockId::new(
                block_id,
                "v1".to_string(),
            )],
            path: "ab/cdef0123456789.text.idx.tar".to_string(),
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
