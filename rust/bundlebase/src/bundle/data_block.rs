use crate::bundle::operation::SourceInfo;
use crate::data::{DataReader, VersionedBlockId};
use crate::index::{
    ColumnIndex, FilterAnalyzer, IndexDefinition, IndexPredicate, IndexSelector, IndexableFilter,
    GLOBAL_INDEX_CACHE,
};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{BlockId, IOReadFile, IOReadWriteDir};
use crate::metrics::{start_span, OperationCategory, OperationOutcome, OperationTimer};
use crate::object_id::ColumnId;
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
use std::collections::HashSet;
use std::sync::Arc;

/// Candidate index for a query with its estimated selectivity
struct IndexCandidate<'a> {
    filter: &'a IndexableFilter,
    selectivity: f64,
    /// The deserialized index, retained from selectivity estimation to avoid double disk reads
    index: Arc<ColumnIndex>,
}

/// A DataBlock is a logical, tablular view of data contained within a single source, regardless of the underlying storage format.
#[derive(Clone, Debug)]
pub struct DataBlock {
    id: BlockId,
    version: String,
    schema: SchemaRef,
    reader: Arc<dyn DataReader>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    data_dir: Arc<dyn IOReadWriteDir>,
    config: Arc<BundleConfig>,
    /// Source information if this block was attached via a source fetch
    source_info: Option<SourceInfo>,
    /// Column IDs for this block's schema fields (positional, matching schema field order)
    column_ids: Vec<ColumnId>,
    /// Row numbers (within this block) that have been deleted via tombstones
    deleted_rows: Arc<RwLock<HashSet<u32>>>,
}

impl DataBlock {
    pub fn table_name(id: &BlockId) -> String {
        format!("__block_{}", id)
    }

    pub fn new(
        id: BlockId,
        schema: SchemaRef,
        version: &str,
        reader: Arc<dyn DataReader>,
        indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
        data_dir: Arc<dyn IOReadWriteDir>,
        config: Arc<BundleConfig>,
        source_info: Option<SourceInfo>,
        column_ids: Vec<ColumnId>,
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
            column_ids,
            deleted_rows: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub fn id(&self) -> &BlockId {
        &self.id
    }

    /// Resolve a physical column name to its ColumnId
    fn column_id_for_physical_name(&self, name: &str) -> Option<ColumnId> {
        self.schema
            .column_with_name(name)
            .and_then(|(idx, _)| self.column_ids.get(idx).copied())
    }

    /// Returns source information if this block was attached via a source fetch
    pub fn source_info(&self) -> Option<&SourceInfo> {
        self.source_info.as_ref()
    }

    /// Returns the column IDs for this block's schema fields
    pub fn column_ids(&self) -> &[ColumnId] {
        &self.column_ids
    }

    /// Add deleted row numbers to this block's tombstone set.
    pub fn add_deleted_rows(&self, rows: impl IntoIterator<Item = u32>) {
        self.deleted_rows.write().extend(rows);
    }

    /// Load index (from cache or disk) and estimate selectivity.
    /// Returns None if the index should be skipped due to high selectivity.
    /// On success returns both the selectivity and the deserialized index
    /// so callers can reuse it without a second disk read.
    async fn check_index_selectivity(
        &self,
        index_path: &str,
        column: &str,
        predicate: &IndexPredicate,
    ) -> Result<Option<(f64, Arc<ColumnIndex>)>, Box<dyn std::error::Error + Send + Sync>> {
        // Check the global cache first
        let index = if let Some(cached) = GLOBAL_INDEX_CACHE.get(index_path) {
            cached
        } else {
            // Load index file from data directory
            let index_file =
                ObjectStoreFile::from_str(index_path, self.data_dir.as_ref(), self.config.clone())?;

            let index_bytes = index_file
                .read_bytes()
                .await?
                .ok_or_else(|| format!("Index file not found: {}", index_path))?;

            // Deserialize and cache the index
            let index = Arc::new(ColumnIndex::deserialize(index_bytes, column.to_string())?);
            GLOBAL_INDEX_CACHE.insert(index_path.to_string(), index.clone());
            index
        };

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

        Ok(Some((selectivity, index)))
    }

    /// Perform index lookup using a pre-loaded `ColumnIndex`.
    fn lookup_index(
        index: &ColumnIndex,
        predicate: &IndexPredicate,
    ) -> Vec<crate::data::RowId> {
        match predicate {
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
        }
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
            // Resolve filter column name to ColumnId via block's column_ids
            let column_id = match self.column_id_for_physical_name(&filter.column) {
                Some(id) => id,
                None => continue, // Column not found in schema, skip
            };

            // Try to find a column index for this filter
            if let Some(index_def) =
                IndexSelector::select_index_from_ref(&column_id, versioned_block, &self.indexes)
            {
                // Skip text indexes — they can't serve column predicates
                if index_def.is_text() {
                    continue;
                }
                // Get the index file path
                if let Some(indexed_blocks) = index_def.indexed_blocks(versioned_block) {
                    let index_path = indexed_blocks.path();

                    // Check selectivity
                    match self
                        .check_index_selectivity(index_path, &filter.column, &filter.predicate)
                        .await
                    {
                        Ok(Some((selectivity, index))) => {
                            // This index is usable - add to candidates
                            log::debug!(
                                "Index candidate on column '{}': selectivity {:.1}%",
                                filter.column,
                                selectivity * 100.0
                            );
                            candidates.push(IndexCandidate {
                                filter,
                                selectivity,
                                index,
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

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> datafusion::common::Result<Vec<datafusion::logical_expr::TableProviderFilterPushDown>> {
        use datafusion::logical_expr::TableProviderFilterPushDown;

        Ok(filters
            .iter()
            .map(|_| TableProviderFilterPushDown::Inexact)
            .collect())
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        let versioned_block = VersionedBlockId::new(self.id, self.version.clone());
        let deleted = self.deleted_rows.read().clone();

        // Try column index optimization
        let indexable_filters = FilterAnalyzer::extract_indexable(filters);

        if !indexable_filters.is_empty() {
            // Evaluate all indexable filters and select the best column index
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

                // Perform lookup using the already-deserialized index (no second disk read)
                let mut row_ids = Self::lookup_index(
                    &best.index,
                    &best.filter.predicate,
                );

                // Remove deleted rows from the inclusion set
                if !deleted.is_empty() {
                    row_ids.retain(|rid| !deleted.contains(&rid.row_number()));
                }

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
            } else {
                // No suitable index found (all had high selectivity or errors)
                log::debug!(
                    "No suitable index found among {} indexable filters (all had high selectivity or errors)",
                    indexable_filters.len()
                );
            }
        }

        // Phase 2: Fall back to full scan
        let source = self.reader
            .data_source(projection, filters, limit, None)
            .await?;

        if deleted.is_empty() {
            let exec = DataSourceExec::new(source.clone());
            Ok(Arc::new(exec))
        } else {
            // Wrap with tombstone filter to exclude deleted rows
            let filtered = crate::bundle::tombstone_filter::TombstoneFilterDataSource::new(
                source,
                Arc::new(deleted),
            );
            let exec = DataSourceExec::new(Arc::new(filtered));
            Ok(Arc::new(exec))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_table_name() {
        let id = BlockId::generate();
        let table = DataBlock::table_name(&id);
        assert!(table.starts_with("__block_"));
        assert_eq!(table.len(), 8 + 16); // "__block_" + 16 hex chars
    }

}
