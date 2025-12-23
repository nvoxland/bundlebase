use crate::data_reader::{DataReader, VersionedBlockId};
use crate::data_storage::{ObjectId, ObjectStoreDir};
use crate::index::{ColumnIndex, FilterAnalyzer, IndexDefinition, IndexPredicate, IndexSelector};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::memory::DataSourceExec;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::TableType;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use parking_lot::RwLock;
use std::any::Any;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct DataBlock {
    id: ObjectId,
    version: String,
    schema: SchemaRef,
    reader: Arc<dyn DataReader>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    data_dir: Arc<ObjectStoreDir>,
}

impl DataBlock {
    pub(crate) fn table_name(id: &ObjectId) -> String {
        format!("__block_{}", id)
    }

    pub(crate) fn parse_id(table_name: &str) -> Option<ObjectId> {
        // Handle both "blocks.__block_xxx" and "__block_xxx" formats
        let name = table_name.strip_prefix("blocks.").unwrap_or(table_name);
        match name.strip_prefix("__block_") {
            Some(id) => ObjectId::try_from(id).ok(),
            None => None
        }
    }

    pub fn new(
        id: ObjectId,
        schema: SchemaRef,
        version: &str,
        reader: Arc<dyn DataReader>,
        indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
        data_dir: Arc<ObjectStoreDir>,
    ) -> Self {
        Self {
            id,
            version: version.to_string(),
            schema,
            reader,
            indexes,
            data_dir,
        }
    }

    /// Returns a reference to the indexes
    pub fn indexes(&self) -> &Arc<RwLock<Vec<Arc<IndexDefinition>>>> {
        &self.indexes
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    /// Load index from disk and perform lookup based on predicate
    async fn load_and_lookup_index(
        &self,
        index_path: &str,
        column: &str,
        predicate: &IndexPredicate,
    ) -> Result<Vec<crate::data_reader::RowId>, Box<dyn std::error::Error + Send + Sync>> {
        // Load index file from data directory
        let index_file = self.data_dir.file(index_path)?;

        let index_bytes = index_file.read_bytes().await?
            .ok_or_else(|| format!("Index file not found: {}", index_path))?;

        // Deserialize the index
        let index = ColumnIndex::deserialize(index_bytes, column.to_string())?;

        // Perform lookup based on predicate type
        let row_ids = match predicate {
            IndexPredicate::Exact(val) => {
                index.lookup_exact(val)
            }
            IndexPredicate::In(vals) => {
                vals.iter()
                    .flat_map(|v| index.lookup_exact(v))
                    .collect()
            }
            IndexPredicate::Range { min, max } => {
                index.lookup_range(min, max)
            }
        };

        Ok(row_ids)
    }

    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    pub fn version(&self) -> String {
        self.version.clone()
    }

    /// Returns the underlying data reader.
    pub fn reader(&self) -> Arc<dyn DataReader> {
        self.reader.clone()
    }
}

#[async_trait]
impl TableProvider for DataBlock {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        // Phase 1: Try index optimization
        if let Some(indexable) = FilterAnalyzer::extract_indexable(filters).first() {
            // Create VersionedBlockId for this block
            let versioned_block = VersionedBlockId::new(self.id.clone(), self.version.clone());

            // Try to find an index for this column and block version
            if let Some(index_def) = IndexSelector::select_index_from_ref(
                &indexable.column,
                &versioned_block,
                &self.indexes,
            ) {
                // Get the IndexedBlocks to find the index file path
                if let Some(indexed_blocks) = index_def.indexed_blocks(&versioned_block) {
                    let index_path = indexed_blocks.path();

                    log::debug!(
                        "Using index on column '{}' for block {} (version {}) at path: {}, projection: {:?}",
                        indexable.column,
                        self.id,
                        self.version,
                        index_path,
                        projection
                    );

                    // Load index from disk and perform lookup
                    match self.load_and_lookup_index(index_path, &indexable.column, &indexable.predicate).await {
                        Ok(row_ids) => {
                            log::debug!(
                                "Index lookup found {} matching rows for column '{}'",
                                row_ids.len(),
                                indexable.column
                            );

                            // Use optimized data source with row IDs
                            let exec = DataSourceExec::new(
                                self.reader
                                    .data_source(projection, filters, limit, Some(&row_ids))
                                    .await?
                                    .clone(),
                            );
                            return Ok(Arc::new(exec));
                        }
                        Err(e) => {
                            // Index loading or lookup failed, fall back to full scan
                            log::warn!(
                                "Index lookup failed for column '{}': {}. Falling back to full scan.",
                                indexable.column,
                                e
                            );
                        }
                    }
                }
            }
        }

        // Phase 2: Fall back to full scan
        let exec = DataSourceExec::new(
            self.reader
                .data_source(projection, filters, limit, None)
                .await?
                .clone(),
        );
        Ok(Arc::new(exec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_table_name() {
        assert_eq!("__block_53", DataBlock::table_name(&ObjectId::from(83)))
    }


    #[test]
    fn test_parse_id() {
        assert_eq!(Some(ObjectId::from(83)), DataBlock::parse_id("__block_53"));
        assert_eq!(None, DataBlock::parse_id("random_table"));
        assert_eq!(None, DataBlock::parse_id("__block_x"));
    }
}