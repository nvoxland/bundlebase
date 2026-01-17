use crate::bundle::operation::SourceInfo;
use crate::data::{DataReader, VersionedBlockId};
use crate::index::{
    ColumnIndex, FilterAnalyzer, IndexDefinition, IndexPredicate, IndexSelector, IndexableFilter,
};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{ObjectId, IOReadFile, IOReadWriteDir};
use crate::metrics::{start_span, OperationCategory, OperationOutcome, OperationTimer};
use crate::BundleConfig;
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

/// Candidate index for a query with its estimated selectivity
struct IndexCandidate<'a> {
    filter: &'a IndexableFilter,
    index_path: String,
    selectivity: f64,
}

/// A DataBlock is a logical, tablular view of data contained within a single source, regardless of the underlying storage format.
#[derive(Clone, Debug)]
pub struct DataBlock {
    id: ObjectId,
    version: String,
    schema: SchemaRef,
    reader: Arc<dyn DataReader>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    data_dir: Arc<dyn IOReadWriteDir>,
    config: Arc<BundleConfig>,
    /// Source information if this block was attached via a source fetch
    source_info: Option<SourceInfo>,
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
            None => None,
        }
    }

    pub fn new(
        id: ObjectId,
        schema: SchemaRef,
        version: &str,
        reader: Arc<dyn DataReader>,
        indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
        data_dir: Arc<dyn IOReadWriteDir>,
        config: Arc<BundleConfig>,
        source_info: Option<SourceInfo>,
    ) -> Self {
        Self {
            id,
            version: version.to_string(),
            schema,
            reader,
            indexes,
            data_dir,
            config,
            source_info,
        }
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    /// Returns source information if this block was attached via a source fetch
    pub fn source_info(&self) -> Option<&SourceInfo> {
        self.source_info.as_ref()
    }

    /// Load index from disk and estimate selectivity
    /// Returns None if the index should be skipped due to high selectivity
    async fn check_index_selectivity(
        &self,
        index_path: &str,
        column: &str,
        predicate: &IndexPredicate,
    ) -> Result<Option<f64>, Box<dyn std::error::Error + Send + Sync>> {
        // Load index file from data directory
        let index_file =
            ObjectStoreFile::from_str(index_path, self.data_dir.as_ref(), self.config.clone())?;

        let index_bytes = index_file
            .read_bytes()
            .await?
            .ok_or_else(|| format!("Index file not found: {}", index_path))?;

        // Deserialize the index
        let index = ColumnIndex::deserialize(index_bytes, column.to_string())?;

        // Estimate selectivity
        let selectivity = index.estimate_selectivity(predicate);

        // Threshold for using index: if selectivity > 20%, full scan is likely faster
        const SELECTIVITY_THRESHOLD: f64 = 0.2;

        if selectivity > SELECTIVITY_THRESHOLD {
            log::info!(
                "Skipping index on column '{}': selectivity {:.1}% exceeds threshold {:.1}% (full scan likely faster)",
                column,
                selectivity * 100.0,
                SELECTIVITY_THRESHOLD * 100.0
            );
            return Ok(None);
        }

        log::debug!(
            "Index selectivity for column '{}': {:.1}% (below threshold, using index)",
            column,
            selectivity * 100.0
        );

        Ok(Some(selectivity))
    }

    /// Load index from disk and perform lookup based on predicate
    async fn load_and_lookup_index(
        &self,
        index_path: &str,
        column: &str,
        predicate: &IndexPredicate,
    ) -> Result<Vec<crate::data::RowId>, Box<dyn std::error::Error + Send + Sync>> {
        // Load index file from data directory
        let index_file =
            ObjectStoreFile::from_str(index_path, self.data_dir.as_ref(), self.config.clone())?;

        let index_bytes = index_file
            .read_bytes()
            .await?
            .ok_or_else(|| format!("Index file not found: {}", index_path))?;

        // Deserialize the index
        let index = ColumnIndex::deserialize(index_bytes, column.to_string())?;

        // Perform lookup based on predicate type
        let row_ids = match predicate {
            IndexPredicate::Exact(val) => index.lookup_exact(val),
            IndexPredicate::In(vals) => {
                // Process IN values in batches to bound memory usage
                // Use HashSet for efficient O(1) deduplication
                use std::collections::HashSet;

                const BATCH_SIZE: usize = 1000;
                let mut unique_row_ids = HashSet::new();

                // Process values in chunks to avoid materializing all lookups at once
                for chunk in vals.chunks(BATCH_SIZE) {
                    for val in chunk {
                        for row_id in index.lookup_exact(val) {
                            unique_row_ids.insert(row_id);
                        }
                    }
                }

                // Convert to Vec and sort for consistent ordering
                let mut row_ids: Vec<_> = unique_row_ids.into_iter().collect();
                row_ids.sort_unstable_by_key(|r| r.as_u64());
                row_ids
            }
            IndexPredicate::Range { min, max } => index.lookup_range(min, max),
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

    /// Evaluate all indexable filters and select the most selective index
    /// Returns None if no suitable index is found or all have selectivity above threshold
    async fn select_best_index<'a>(
        &self,
        indexable_filters: &'a [IndexableFilter],
        versioned_block: &VersionedBlockId,
    ) -> Option<IndexCandidate<'a>> {
        let mut candidates = Vec::new();

        // Evaluate each indexable filter
        for filter in indexable_filters {
            // Try to find an index for this column
            if let Some(index_def) =
                IndexSelector::select_index_from_ref(&filter.column, versioned_block, &self.indexes)
            {
                // Get the index file path
                if let Some(indexed_blocks) = index_def.indexed_blocks(versioned_block) {
                    let index_path = indexed_blocks.path();

                    // Check selectivity
                    match self
                        .check_index_selectivity(index_path, &filter.column, &filter.predicate)
                        .await
                    {
                        Ok(Some(selectivity)) => {
                            // This index is usable - add to candidates
                            log::debug!(
                                "Index candidate on column '{}': selectivity {:.1}%",
                                filter.column,
                                selectivity * 100.0
                            );
                            candidates.push(IndexCandidate {
                                filter,
                                index_path: index_path.to_string(),
                                selectivity,
                            });
                        }
                        Ok(None) => {
                            // Selectivity too high - skip this index
                            log::debug!(
                                "Skipping index on column '{}' (selectivity too high)",
                                filter.column
                            );
                        }
                        Err(e) => {
                            // Selectivity check failed - skip this index
                            log::debug!(
                                "Skipping index on column '{}' (selectivity check failed: {})",
                                filter.column,
                                e
                            );
                        }
                    }
                }
            }
        }

        // Choose the index with the lowest selectivity (most selective)
        candidates.into_iter().min_by(|a, b| {
            a.selectivity
                .partial_cmp(&b.selectivity)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
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
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        // Phase 1: Try index optimization
        let indexable_filters = FilterAnalyzer::extract_indexable(filters);

        if !indexable_filters.is_empty() {
            // Create VersionedBlockId for this block
            let versioned_block = VersionedBlockId::new(self.id, self.version.clone());

            // Evaluate all indexable filters and select the best index
            if let Some(best) = self
                .select_best_index(&indexable_filters, &versioned_block)
                .await
            {
                // Start span and timer for index lookup
                let mut span = start_span(OperationCategory::Index, "lookup");
                span.set_attribute("column", &best.filter.column);
                span.set_attribute("selectivity", format!("{:.3}", best.selectivity));
                span.set_attribute("block_id", self.id.to_string());

                let timer = OperationTimer::start(OperationCategory::Index, "lookup")
                    .with_label("column", &best.filter.column);

                log::info!(
                    "Selected index on column '{}' with selectivity {:.1}% (best among {} candidates)",
                    best.filter.column,
                    best.selectivity * 100.0,
                    indexable_filters.len()
                );

                log::debug!(
                    "Using index on column '{}' for block {} (version {}), projection: {:?}",
                    best.filter.column,
                    self.id,
                    self.version,
                    projection
                );

                // Load index from disk and perform lookup
                match self
                    .load_and_lookup_index(
                        &best.index_path,
                        &best.filter.column,
                        &best.filter.predicate,
                    )
                    .await
                {
                    Ok(row_ids) => {
                        log::debug!(
                            "Index lookup found {} matching rows for column '{}'",
                            row_ids.len(),
                            best.filter.column
                        );

                        // Record successful index hit
                        span.set_attribute("matched_rows", row_ids.len().to_string());
                        span.set_outcome(OperationOutcome::Success);
                        timer.finish(OperationOutcome::Success);

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
                            best.filter.column,
                            e
                        );
                        // Record index error and fallback
                        span.record_error(&e.to_string());
                        timer.finish(OperationOutcome::Error);
                    }
                }
            } else {
                // No suitable index found (all had high selectivity or errors)
                log::debug!(
                    "No suitable index found among {} indexable filters (all had high selectivity or errors)",
                    indexable_filters.len()
                );
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
