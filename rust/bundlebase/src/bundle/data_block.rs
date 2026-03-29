use crate::bundle::block_cache::GLOBAL_BLOCK_CACHE;
use crate::bundle::column_metadata;
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
use datafusion::datasource::memory::MemorySourceConfig;
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
    /// Schema with stable `col_<id>` field names, used by the TableProvider interface.
    /// Built from the stored physical schema; updated with actual reader types on first scan.
    schema: Arc<parking_lot::RwLock<SchemaRef>>,
    reader: Arc<dyn DataReader>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    data_dir: Arc<dyn IOReadWriteDir>,
    config: Arc<BundleConfig>,
    /// Source information if this block was attached via a source fetch
    source_info: Option<SourceInfo>,
    /// Column IDs for this block's schema fields (positional, matching schema field order)
    column_ids: Vec<ColumnId>,
    /// Row numbers (within this block) that have been deleted
    deleted_rows: Arc<RwLock<Vec<u32>>>,
    /// Update overlays to apply at scan time
    update_overlays: Arc<RwLock<Vec<crate::bundle::update_overlay::UpdateOverlay>>>,
    /// Whether this block's version has been validated (first scan reads through reader).
    version_validated: Arc<std::sync::atomic::AtomicBool>,
}

impl DataBlock {
    pub fn table_name(id: &BlockId) -> String {
        format!("__block_{}", id)
    }

    pub fn new(
        id: BlockId,
        physical_schema: SchemaRef,
        version: &str,
        reader: Arc<dyn DataReader>,
        indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
        data_dir: Arc<dyn IOReadWriteDir>,
        config: Arc<BundleConfig>,
        source_info: Option<SourceInfo>,
        column_ids: Vec<ColumnId>,
    ) -> Self {
        // Build ID-based schema: rename each field to `col_<column_id>`
        let id_fields: Vec<Arc<arrow_schema::Field>> = physical_schema
            .fields()
            .iter()
            .zip(column_ids.iter())
            .map(|(field, col_id)| {
                Arc::new(field.as_ref().clone().with_name(column_metadata::col_id_name(col_id)))
            })
            .collect();
        let schema = Arc::new(arrow_schema::Schema::new_with_metadata(
            id_fields,
            physical_schema.metadata().clone(),
        ));

        Self {
            id,
            version: version.to_string(),
            schema: Arc::new(parking_lot::RwLock::new(schema)),
            reader,
            indexes,
            data_dir,
            config,
            source_info,
            column_ids,
            deleted_rows: Arc::new(RwLock::new(Vec::new())),
            update_overlays: Arc::new(RwLock::new(Vec::new())),
            version_validated: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn id(&self) -> &BlockId {
        &self.id
    }

    /// Rename batches from physical column names to stable `col_<id>` names.
    /// Uses the batch's actual field types to build the `col_<id>` schema.
    fn rename_batches_with_col_ids(
        batches: Vec<arrow::record_batch::RecordBatch>,
        column_ids: &[ColumnId],
    ) -> Vec<arrow::record_batch::RecordBatch> {
        batches
            .into_iter()
            .map(|batch| {
                let batch_schema = batch.schema();
                let id_fields: Vec<Arc<arrow_schema::Field>> = batch_schema
                    .fields()
                    .iter()
                    .zip(column_ids.iter())
                    .map(|(field, col_id)| {
                        Arc::new(field.as_ref().clone().with_name(column_metadata::col_id_name(col_id)))
                    })
                    .collect();
                let id_schema = Arc::new(arrow_schema::Schema::new_with_metadata(
                    id_fields,
                    batch_schema.metadata().clone(),
                ));
                arrow::record_batch::RecordBatch::try_new(
                    id_schema,
                    batch.columns().to_vec(),
                )
                .unwrap_or(batch)
            })
            .collect()
    }

    /// Returns source information if this block was attached via a source fetch
    pub fn source_info(&self) -> Option<&SourceInfo> {
        self.source_info.as_ref()
    }

    /// Returns the column IDs for this block's schema fields
    pub fn column_ids(&self) -> &[ColumnId] {
        &self.column_ids
    }

    /// Cache key for this block.
    ///
    /// Uses the data URL + version hash. The version hash is a SHA-256 of the
    /// file contents at commit time, so if the file changes the hash won't
    /// match and we'll get a different cache key on the next commit.
    /// For uncommitted data (version "TEMP"), includes the data_dir URL
    /// to prevent cross-instance collisions.
    fn cache_key(&self) -> String {
        format!("{}:{}:{}", self.data_dir.url(), self.reader.url(), self.version)
    }

    /// Add deleted row numbers to this block's deleted set.
    ///
    /// The block cache is NOT invalidated here — cached batches are the base
    /// data, and deleted rows are applied as a filter on top at query time.
    pub fn add_deleted_rows(&self, rows: impl IntoIterator<Item = u32>) {
        let mut deleted = self.deleted_rows.write();
        deleted.extend(rows);
        deleted.sort_unstable();
        deleted.dedup();
    }

    /// Add an update overlay to this block.
    ///
    /// The block cache is NOT invalidated here — cached batches are the base
    /// data, and overlays are applied as a filter on top at query time.
    pub fn add_update_overlay(&self, overlay: crate::bundle::update_overlay::UpdateOverlay) {
        self.update_overlays.write().push(overlay);
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
        self.schema.read().clone()
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
            // Resolve filter column name (col_<id> format) to ColumnId
            let column_id = match column_metadata::parse_col_id_name(&filter.column) {
                Some(id) => id,
                None => continue, // Not a col_<id> name, skip
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
        self.schema.read().clone()
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
        state: &dyn Session,
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
                    row_ids.retain(|rid| deleted.binary_search(&rid.row_number()).is_err());
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

                // Use optimized data source with row IDs, wrapped for col_<id> renaming
                let inner_source = self.reader
                    .data_source(projection, filters, limit, Some(&row_ids))
                    .await?
                    .clone();
                let projected_col_ids: Vec<ColumnId> = match projection {
                    Some(proj) => proj.iter().filter_map(|&i| self.column_ids.get(i).copied()).collect(),
                    None => self.column_ids.clone(),
                };
                // Use self.schema for planning (types may not match reader exactly, but
                // SchemaRenameDataSource will use actual batch types at runtime)
                let current_schema = self.schema.read().clone();
                let projected_schema = match projection {
                    Some(proj) => {
                        let fields: Vec<_> = proj.iter()
                            .filter_map(|&i| current_schema.fields().get(i).cloned())
                            .collect();
                        Arc::new(arrow_schema::Schema::new(fields))
                    }
                    None => current_schema,
                };
                let source = Arc::new(
                    crate::bundle::schema_rename_filter::SchemaRenameDataSource::new(
                        inner_source,
                        projected_schema,
                        projected_col_ids,
                    ),
                );
                let exec = DataSourceExec::new(source);
                return Ok(Arc::new(exec));
            } else {
                // No suitable index found (all had high selectivity or errors)
                log::debug!(
                    "No suitable index found among {} indexable filters (all had high selectivity or errors)",
                    indexable_filters.len()
                );
            }
        }

        // Phase 2: Full scan with block cache
        let overlays = self.update_overlays.read().clone();
        let cache_key = self.cache_key();
        let validated = self.version_validated.load(std::sync::atomic::Ordering::Relaxed);

        // Try to serve from the block cache (stores base data with col_<id> names).
        // Only use cache after version has been validated (first scan reads through reader).
        let mut source: Arc<dyn datafusion::datasource::source::DataSource> =
            if validated {
                if let Some(cached) = GLOBAL_BLOCK_CACHE.get(&cache_key) {
                    log::debug!("Block cache hit for {}", cache_key);
                    // Use the cached batch's own schema (derived from the reader's actual types)
                    let batch_schema = cached.batches.first()
                        .map(|b| b.schema())
                        .unwrap_or_else(|| self.schema.read().clone());
                    Arc::new(MemorySourceConfig::try_new(
                        &[cached.batches.as_ref().clone()],
                        batch_schema,
                        projection.cloned(),
                    )?)
                } else {
                    // Validated but not cached (evicted or first scan after validation).
                    // Read through reader, rename to col_<id>, cache result.
                    let base_source = self.reader
                        .data_source(None, &[], None, None)
                        .await?;
                    let task_ctx = Arc::new(
                        datafusion::execution::TaskContext::default()
                            .with_runtime(Arc::clone(state.runtime_env())),
                    );
                    let stream = base_source.open(0, task_ctx)?;
                    let batches: Vec<arrow::record_batch::RecordBatch> =
                        datafusion::physical_plan::common::collect(stream).await?;
                    let batches = Self::rename_batches_with_col_ids(batches, &self.column_ids);
                    let batch_schema = batches.first()
                        .map(|b| b.schema())
                        .unwrap_or_else(|| self.schema.read().clone());
                    GLOBAL_BLOCK_CACHE.insert(cache_key.clone(), batches.clone());
                    Arc::new(MemorySourceConfig::try_new(
                        &[batches],
                        batch_schema,
                        projection.cloned(),
                    )?)
                }
            } else {
                // First scan: read through reader (validates version), rename, then cache.
                let base_source = self.reader
                    .data_source(None, &[], None, None)
                    .await?;
                let task_ctx = Arc::new(
                    datafusion::execution::TaskContext::default()
                        .with_runtime(Arc::clone(state.runtime_env())),
                );
                let stream = base_source.open(0, task_ctx)?;
                let batches: Vec<arrow::record_batch::RecordBatch> =
                    datafusion::physical_plan::common::collect(stream).await?;
                let batches = Self::rename_batches_with_col_ids(batches, &self.column_ids);
                let batch_schema = batches.first()
                    .map(|b| b.schema())
                    .unwrap_or_else(|| self.schema.read().clone());

                // Version validated successfully — mark and cache.
                // Update self.schema with actual reader types for consistent planning.
                self.version_validated.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(first) = batches.first() {
                    *self.schema.write() = first.schema();
                }
                GLOBAL_BLOCK_CACHE.insert(cache_key.clone(), batches.clone());
                Arc::new(MemorySourceConfig::try_new(
                    &[batches],
                    batch_schema,
                    projection.cloned(),
                )?)
            };

        // Apply deleted row filter if there are deleted rows
        if !deleted.is_empty() {
            source = Arc::new(crate::bundle::deleted_row_filter::DeletedRowFilterDataSource::new(
                source,
                Arc::new(deleted),
            ));
        }

        // Apply update overlay if there are updates for this block
        if !overlays.is_empty() {
            // Build projected column_ids matching the scan output columns
            let projected_col_ids = match projection {
                Some(proj) => proj.iter().filter_map(|&i| self.column_ids.get(i).copied()).collect::<Vec<_>>(),
                None => self.column_ids.clone(),
            };
            let current_schema = self.schema.read().clone();
            let projected_schema = match projection {
                Some(proj) => {
                    let fields: Vec<_> = proj.iter()
                        .filter_map(|&i| current_schema.fields().get(i).cloned())
                        .collect();
                    Arc::new(arrow_schema::Schema::new(fields))
                }
                None => current_schema,
            };
            let overlay_source = crate::bundle::update_overlay_filter::UpdateOverlayDataSource::new(
                source.clone(),
                &overlays,
                &projected_col_ids,
                &projected_schema,
            );
            if overlay_source.has_updates() {
                source = Arc::new(overlay_source);
            }
        }

        let exec = DataSourceExec::new(source.clone());
        Ok(Arc::new(exec))
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
