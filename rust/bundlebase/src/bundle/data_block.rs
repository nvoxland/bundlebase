use crate::bundle::block_cache::GLOBAL_BLOCK_CACHE;
use crate::bundle::bundle_schema;
use crate::bundle::operation::SourceInfo;
use crate::data::{DataReader, VersionedBlockId};
use crate::index::{
    BTreeIndex, FilterAnalyzer, IndexDefinition, IndexPredicate, IndexSelector, IndexableFilter,
    GLOBAL_INDEX_CACHE,
};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{BlockId, IOReadFile, IOReadWriteDir};
use crate::metrics::{
    record_cache_operation, record_operation, start_span, KeyValue, OperationCategory,
    OperationOutcome, OperationTimer,
};
use crate::object_id::ColumnId;
use crate::progress::ProgressScope;
use crate::BundleConfig;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::memory::DataSourceExec;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::memory::MemorySourceConfig;
use datafusion::datasource::TableType;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use futures::StreamExt;
use parking_lot::RwLock;
use std::any::Any;
use std::sync::Arc;

/// Candidate index for a query with its estimated selectivity
struct IndexCandidate<'a> {
    filter: &'a IndexableFilter,
    selectivity: f64,
    /// The deserialized index, retained from selectivity estimation to avoid double disk reads
    index: Arc<BTreeIndex>,
}

/// A DataBlock is a logical, tablular view of data contained within a single source, regardless of the underlying storage format.
#[derive(Clone, Debug)]
pub struct DataBlock {
    id: BlockId,
    version: String,
    /// Schema with stable internal name fields, used by the TableProvider interface.
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
    /// Count of narrow-projection bypasses served for this block. Used to
    /// promote hot blocks into the full block cache after repeated narrow
    /// queries — see Phase 2.6 in `scan()`.
    narrow_bypass_count: Arc<std::sync::atomic::AtomicU32>,
    /// DataFusion statistics cached after the first column-stats load. Starts as None;
    /// populated during can_prune_block() so the optimizer gets stats on subsequent queries.
    cached_df_statistics: Arc<RwLock<Option<datafusion::common::Statistics>>>,
    /// Row count captured at attach time (from AttachBlockOp). Used to seed
    /// statistics() so DataFusion's optimizer has cardinality without I/O.
    num_rows: Option<usize>,
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
        num_rows: Option<usize>,
    ) -> Self {
        // Build ID-based schema: rename each field to `col_<column_id>`
        let id_fields: Vec<Arc<arrow_schema::Field>> = physical_schema
            .fields()
            .iter()
            .zip(column_ids.iter())
            .map(|(field, col_id)| {
                Arc::new(
                    field
                        .as_ref()
                        .clone()
                        .with_name(bundle_schema::generate_internal_name(col_id)),
                )
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
            narrow_bypass_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            cached_df_statistics: Arc::new(RwLock::new(None)),
            num_rows,
        }
    }

    pub fn id(&self) -> &BlockId {
        &self.id
    }

    /// Row count captured at attach time, if known.
    pub fn num_rows(&self) -> Option<usize> {
        self.num_rows
    }

    /// Rename batches from physical column names to stable internal names.
    /// Uses the batch's actual field types to build the internal name schema.
    fn rename_batches_with_internal_names(
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
                        Arc::new(
                            field
                                .as_ref()
                                .clone()
                                .with_name(bundle_schema::generate_internal_name(col_id)),
                        )
                    })
                    .collect();
                let id_schema = Arc::new(arrow_schema::Schema::new_with_metadata(
                    id_fields,
                    batch_schema.metadata().clone(),
                ));
                arrow::record_batch::RecordBatch::try_new(id_schema, batch.columns().to_vec())
                    .unwrap_or(batch)
            })
            .collect()
    }

    /// Build a synthetic ExecutionPlan that returns `rows` empty rows (zero
    /// columns). Used by the COUNT(*) fast path in `scan()` — DataFusion's
    /// count aggregator only needs row counts when projection is empty, so
    /// we can satisfy the query without touching the underlying file.
    fn empty_projection_plan(
        &self,
        rows: usize,
    ) -> datafusion::common::Result<datafusion::catalog::memory::DataSourceExec> {
        use arrow::record_batch::{RecordBatch, RecordBatchOptions};
        use arrow_schema::Schema;
        use datafusion::catalog::memory::DataSourceExec;
        use datafusion::datasource::memory::MemorySourceConfig;

        let empty_schema = Arc::new(Schema::empty());
        // Chunk the synthetic rows into reasonable batch sizes so downstream
        // operators see normal-shaped batches rather than one giant batch.
        const BATCH_ROWS: usize = 8192;
        let mut batches: Vec<RecordBatch> = Vec::new();
        let mut remaining = rows;
        while remaining > 0 {
            let take = remaining.min(BATCH_ROWS);
            let opts = RecordBatchOptions::default().with_row_count(Some(take));
            let batch = RecordBatch::try_new_with_options(empty_schema.clone(), vec![], &opts)
                .map_err(|e| datafusion::common::DataFusionError::ArrowError(Box::new(e), None))?;
            batches.push(batch);
            remaining -= take;
        }
        let source: Arc<dyn datafusion::datasource::source::DataSource> =
            Arc::new(MemorySourceConfig::try_new(&[batches], empty_schema, None)?);
        Ok(DataSourceExec::new(source))
    }

    /// Returns source information if this block was attached via a source fetch
    pub fn source_info(&self) -> Option<&SourceInfo> {
        self.source_info.as_ref()
    }

    /// Returns the column IDs for this block's schema fields
    pub fn column_ids(&self) -> &[ColumnId] {
        &self.column_ids
    }

    /// Return pre-computed statistics for a specific column, if available.
    ///
    /// Loads stats from the reader's layout file (CSV/JSONL only). Returns `None`
    /// if the column is not in this block, or if this format has no pre-computed stats.
    pub async fn column_stats_for(
        &self,
        column_id: ColumnId,
    ) -> Result<Option<bundlebase_data::page_map::ColumnStats>, crate::BundlebaseError> {
        let idx = match self.column_ids.iter().position(|id| *id == column_id) {
            Some(i) => i,
            None => return Ok(None),
        };
        let all_stats = self.reader.column_stats().await?;
        Ok(all_stats.into_iter().nth(idx))
    }

    /// Check that the source file version still matches the version stored
    /// on this block at attach time. Returns `Err(Version mismatch …)` if
    /// the file has changed since the bundle was committed.
    ///
    /// Paths that actually open the data file trigger version validation
    /// automatically via `VersionedObjectStoreFile`; this helper is for the
    /// code paths that skip reading the data (COUNT(*) fast path, metadata-
    /// only queries) where stale data would otherwise silently return the
    /// old answer. Once validated, the `version_validated` flag short-
    /// circuits subsequent calls so this is not on the hot path after the
    /// first query.
    pub async fn validate_version(&self) -> Result<(), crate::BundlebaseError> {
        if self
            .version_validated
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(());
        }
        let current = self.reader.read_version().await?;
        if current != self.version {
            return Err(crate::BundlebaseError::from(format!(
                "Version mismatch for '{}': expected '{}', found '{}'. \
                 The source file has changed since the bundle was created.",
                self.reader.url(),
                self.version,
                current
            )));
        }
        self.version_validated
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Returns true if the given filters provably exclude this entire block, based on
    /// pre-computed column statistics. Conservative: returns false when uncertain.
    ///
    /// Also populates `cached_df_statistics` as a side effect so DataFusion's optimizer
    /// has accurate cardinality estimates for join planning on subsequent queries.
    async fn can_prune_block(
        &self,
        filters: &[IndexableFilter],
    ) -> datafusion::common::Result<bool> {
        use crate::index::IndexPredicate;
        use bundlebase_data::page_filter::{prune_block_exact, prune_block_range, prune_prefix};

        // Load all column stats once; we'll use them for both pruning and caching DF stats.
        let all_stats = self
            .reader
            .column_stats()
            .await
            .map_err(|e| datafusion::common::DataFusionError::External(e))?;

        // Cache DataFusion statistics (column cardinality / min / max) for the optimizer.
        // Hold the write lock for the entire check-and-set to avoid redundant concurrent builds.
        if !all_stats.is_empty() {
            let mut cache = self.cached_df_statistics.write();
            if cache.is_none() {
                *cache = Some(build_df_statistics(
                    &all_stats,
                    &self.schema.read(),
                    self.num_rows,
                ));
            }
        }

        for filter in filters {
            // Look up column ID by name
            let col_pos = match self
                .schema
                .read()
                .fields()
                .iter()
                .position(|f| f.name() == &filter.column)
            {
                Some(p) => p,
                None => continue,
            };
            let stats = match all_stats.get(col_pos) {
                Some(s) => s.clone(),
                None => continue, // No stats for this column — can't prune
            };

            let can_prune = match &filter.predicate {
                IndexPredicate::Exact(val) => {
                    prune_block_exact(val, stats.min.as_ref(), stats.max.as_ref())
                }
                IndexPredicate::Range {
                    min: fmin,
                    max: fmax,
                } => prune_block_range(fmin, fmax, stats.min.as_ref(), stats.max.as_ref()),
                IndexPredicate::In(vals) => {
                    // Prune only if every value in the IN list is outside the block range.
                    stats.min.is_some()
                        && stats.max.is_some()
                        && vals
                            .iter()
                            .all(|v| prune_block_exact(v, stats.min.as_ref(), stats.max.as_ref()))
                }
                IndexPredicate::IsNull => {
                    // No nulls in this block — IS NULL can't match
                    stats.null_count == 0
                }
                IndexPredicate::IsNotNull => {
                    // Block has values (min/max present) — can't prune
                    false
                }
                IndexPredicate::Prefix(prefix) => {
                    prune_prefix(prefix, stats.min.as_ref(), stats.max.as_ref())
                }
            };

            if can_prune {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Cache key for this block.
    ///
    /// Uses the data URL + version hash. The version hash is a SHA-256 of the
    /// file contents at commit time, so if the file changes the hash won't
    /// match and we'll get a different cache key on the next commit.
    /// For uncommitted data (version "TEMP"), includes the data_dir URL
    /// to prevent cross-instance collisions.
    fn cache_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.data_dir.url(),
            self.reader.url(),
            self.version
        )
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
    ) -> Result<Option<(f64, Arc<BTreeIndex>)>, Box<dyn std::error::Error + Send + Sync>> {
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
            let index = Arc::new(BTreeIndex::deserialize(index_bytes, column.to_string())?);
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

    /// Perform index lookup using a pre-loaded `BTreeIndex`.
    fn lookup_index(index: &BTreeIndex, predicate: &IndexPredicate) -> Vec<crate::data::RowId> {
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
            // IsNull, IsNotNull, and Prefix are not handled by the column index
            IndexPredicate::IsNull | IndexPredicate::IsNotNull | IndexPredicate::Prefix(_) => {
                vec![]
            }
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

    /// Collect a RecordBatch stream into a Vec, reporting progress per batch.
    ///
    /// Opens a `ProgressScope` named after the reader URL so that slow remote reads
    /// (S3, GCS, SFTP, etc.) are visible during SELECT queries, not just during FETCH.
    async fn collect(
        reader_url: &url::Url,
        mut stream: datafusion::execution::SendableRecordBatchStream,
    ) -> datafusion::common::Result<Vec<arrow::record_batch::RecordBatch>> {
        let progress = ProgressScope::new(&format!("Reading {}", reader_url), None);
        let mut batches = Vec::new();
        let mut rows_read: u64 = 0;
        while let Some(result) = stream.next().await {
            let batch = result?;
            rows_read += batch.num_rows() as u64;
            progress.update(rows_read, Some("rows"));
            batches.push(batch);
        }
        Ok(batches)
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
            // Resolve filter column internal name to ColumnId
            let column_id = match bundle_schema::parse_internal_name(&filter.column) {
                Some(id) => id,
                None => continue, // Not an internal name, skip
            };

            // Try to find a column index for this filter
            if let Some(index_def) =
                IndexSelector::select_index_from_ref(&column_id, versioned_block, &self.indexes)
            {
                // Skip text indexes — they can't serve column predicates
                if index_def.is_inverted() {
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
    ) -> datafusion::common::Result<Vec<datafusion::logical_expr::TableProviderFilterPushDown>>
    {
        use datafusion::logical_expr::TableProviderFilterPushDown;

        Ok(filters
            .iter()
            .map(|_| TableProviderFilterPushDown::Inexact)
            .collect())
    }

    fn statistics(&self) -> Option<datafusion::common::Statistics> {
        use datafusion::common::stats::Precision;
        use datafusion::common::Statistics;

        if let Some(cached) = self.cached_df_statistics.read().clone() {
            return Some(cached);
        }
        // No column stats loaded yet, but we may still know the row count from
        // the attach-time metadata. Return a minimal Statistics so the optimizer
        // can plan COUNT(*) and similar without scanning data.
        self.num_rows.map(|n| Statistics {
            num_rows: Precision::Exact(n),
            total_byte_size: Precision::Absent,
            column_statistics: self
                .schema
                .read()
                .fields()
                .iter()
                .map(|_| datafusion::common::ColumnStatistics::new_unknown())
                .collect(),
        })
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

        // Phase 0: Empty-projection fast path (e.g. `SELECT COUNT(*)`).
        // DataFusion asks for zero columns when it only needs row counts.
        // If we know `num_rows` from attach metadata and there's nothing
        // that could change the row count (no filters, no overlays, no
        // deleted rows), return a synthetic plan with N empty rows so the
        // count aggregator never touches the underlying file.
        //
        // Version validation still has to happen though — otherwise a
        // modified source file can silently return the stale row count.
        // `VersionedObjectStoreFile` only fires when the data file is
        // actually read, and we're specifically skipping that. Call
        // `validate_version` explicitly so a version mismatch surfaces as
        // a query error instead of silently-succeeding stale data.
        if let (Some(proj), Some(rows)) = (projection, self.num_rows) {
            if proj.is_empty()
                && filters.is_empty()
                && deleted.is_empty()
                && self.update_overlays.read().is_empty()
            {
                self.validate_version()
                    .await
                    .map_err(datafusion::common::DataFusionError::External)?;
                let effective_rows = match limit {
                    Some(lim) => rows.min(lim),
                    None => rows,
                };
                return Ok(Arc::new(self.empty_projection_plan(effective_rows)?));
            }
        }

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

                log::debug!(
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
                let mut row_ids = Self::lookup_index(&best.index, &best.filter.predicate);

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

                // Use optimized data source with row IDs, wrapped for internal name renaming
                let inner_source = self
                    .reader
                    .data_source(projection, filters, limit, Some(&row_ids))
                    .await?
                    .clone();
                let projected_col_ids: Vec<ColumnId> = match projection {
                    Some(proj) => proj
                        .iter()
                        .filter_map(|&i| self.column_ids.get(i).copied())
                        .collect(),
                    None => self.column_ids.clone(),
                };
                // Use self.schema for planning (types may not match reader exactly, but
                // SchemaRenameDataSource will use actual batch types at runtime)
                let current_schema = self.schema.read().clone();
                let projected_schema = match projection {
                    Some(proj) => {
                        let fields: Vec<_> = proj
                            .iter()
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

        // Phase 2: Stats-based optimizations (block pruning + page filtering).
        // Skipped when update overlays are present — updates can introduce values
        // outside the original stats range, so pruning could return incorrect results.
        let overlays = self.update_overlays.read().clone();
        if overlays.is_empty() && !indexable_filters.is_empty() {
            let current_schema = self.schema.read().clone();
            let projected_schema = match projection {
                Some(proj) => {
                    let fields: Vec<_> = proj
                        .iter()
                        .filter_map(|&i| current_schema.fields().get(i).cloned())
                        .collect();
                    Arc::new(arrow_schema::Schema::new(fields))
                }
                None => current_schema,
            };

            // Block-level pruning: if any filter provably excludes this entire block, return empty.
            if self.can_prune_block(&indexable_filters).await? {
                log::debug!(
                    "Block {} pruned by column stats (filter can't match)",
                    self.id
                );
                record_operation(
                    OperationCategory::Select,
                    OperationOutcome::Skipped,
                    "block_prune",
                    &[KeyValue::new("block_id", self.id.to_string())],
                );
                let exec = datafusion::physical_plan::empty::EmptyExec::new(projected_schema);
                return Ok(Arc::new(exec));
            }

            // Page-level filtering: read only pages whose per-page stats overlap the filters.
            // Only attempted on cache misses — a cached block is already memory-local and
            // scanning it is fast enough that the overhead of page filtering isn't worthwhile.
            let cache_key = self.cache_key();
            let cached = GLOBAL_BLOCK_CACHE.get(&cache_key);
            if cached.is_none() {
                if let Some(page_source) = self
                    .reader
                    .data_source_filtered_pages(projection, filters, limit)
                    .await
                    .map_err(|e| datafusion::common::DataFusionError::External(e))?
                {
                    log::debug!(
                        "Block {} using page-filtered read (bypassing block cache)",
                        self.id
                    );
                    record_operation(
                        OperationCategory::Select,
                        OperationOutcome::Success,
                        "page_filter",
                        &[KeyValue::new("block_id", self.id.to_string())],
                    );
                    let deleted = self.deleted_rows.read().clone();
                    let mut source = page_source;
                    if !deleted.is_empty() {
                        source = Arc::new(
                            crate::bundle::deleted_row_filter::DeletedRowFilterDataSource::new(
                                source,
                                Arc::new(deleted),
                            ),
                        );
                    }
                    return Ok(Arc::new(datafusion::catalog::memory::DataSourceExec::new(
                        source,
                    )));
                }
            }
        }

        // Phase 2.6: Narrow-projection bypass. When the caller wants a strict
        // subset of columns (common for filter / aggregation queries like
        // `SELECT type, COUNT(*) FROM bundle WHERE type = 'user'`), bypass
        // the block cache and push projection through to the reader. The
        // cache-population path below collects the ENTIRE block (all
        // columns), which is pure waste when only one column is needed.
        //
        // Tradeoff: this query's data isn't cached, so a subsequent query
        // on the same block re-reads from disk. For narrow queries the
        // re-read is cheap (projection is pushed all the way down into the
        // CSV / JSONL row parser), and the warm wide-query case still hits
        // the cache because SELECT * takes the Phase 3 path below.
        //
        // Hot-block promotion: only the FIRST narrow query on a block takes
        // the bypass. A second narrow query on the same block means the
        // block is hot — fall through to the Phase 3 path so the block
        // gets cached and subsequent narrow queries hit the cache instead
        // of re-reading from disk on every call.
        //
        // Skipped when overlays exist (they can change per-column data) or
        // when the block is already cached (hit path below is fast enough).
        if let Some(proj) = projection {
            let current_schema = self.schema.read().clone();
            // A 0-column projection is a COUNT(*) — Phase 0 handles that when
            // num_rows is known; when it isn't (e.g. after replace_block), the
            // empty projection must fall through to the full Phase 3 path so
            // the row count is read from the file.  Treat it as non-narrow.
            let narrow = !proj.is_empty() && proj.len() < current_schema.fields().len();
            let first_bypass = self
                .narrow_bypass_count
                .load(std::sync::atomic::Ordering::Relaxed)
                == 0;
            if narrow
                && first_bypass
                && overlays.is_empty()
                && GLOBAL_BLOCK_CACHE.get(&self.cache_key()).is_none()
            {
                self.narrow_bypass_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                log::debug!(
                    "Block {} using narrow-projection bypass ({} of {} columns)",
                    self.id,
                    proj.len(),
                    current_schema.fields().len()
                );
                record_operation(
                    OperationCategory::Select,
                    OperationOutcome::Success,
                    "narrow_projection_bypass",
                    &[KeyValue::new("block_id", self.id.to_string())],
                );
                // Validate source version before the bypass read: this path
                // skips the block cache's first-scan version check, so an
                // out-of-date source file would otherwise return stale data
                // silently.
                self.validate_version()
                    .await
                    .map_err(datafusion::common::DataFusionError::External)?;
                let inner_source = self
                    .reader
                    .data_source(projection, &[], limit, None)
                    .await?;
                let projected_col_ids: Vec<ColumnId> = proj
                    .iter()
                    .filter_map(|&i| self.column_ids.get(i).copied())
                    .collect();
                let projected_schema = {
                    let fields: Vec<_> = proj
                        .iter()
                        .filter_map(|&i| current_schema.fields().get(i).cloned())
                        .collect();
                    Arc::new(arrow_schema::Schema::new(fields))
                };
                let mut source: Arc<dyn datafusion::datasource::source::DataSource> = Arc::new(
                    crate::bundle::schema_rename_filter::SchemaRenameDataSource::new(
                        inner_source,
                        projected_schema,
                        projected_col_ids,
                    ),
                );
                if !deleted.is_empty() {
                    source = Arc::new(
                        crate::bundle::deleted_row_filter::DeletedRowFilterDataSource::new(
                            source,
                            Arc::new(deleted),
                        ),
                    );
                }
                return Ok(Arc::new(DataSourceExec::new(source)));
            }
        }

        // Phase 2.5: Limit fast path. When the caller asked for a small slice
        // (e.g. SELECT * LIMIT 1000), bypass the block cache and push the
        // limit through to the underlying reader. The cache-population path
        // below collects the entire block before applying the limit, which
        // turns a tiny preview into a full scan. Skipped when overlays exist
        // (those can change row counts) or when index/page paths already
        // returned above.
        if let Some(lim) = limit {
            if overlays.is_empty() {
                let inner_source = self
                    .reader
                    .data_source(projection, &[], Some(lim), None)
                    .await?;
                let projected_col_ids: Vec<ColumnId> = match projection {
                    Some(proj) => proj
                        .iter()
                        .filter_map(|&i| self.column_ids.get(i).copied())
                        .collect(),
                    None => self.column_ids.clone(),
                };
                let current_schema = self.schema.read().clone();
                let projected_schema = match projection {
                    Some(proj) => {
                        let fields: Vec<_> = proj
                            .iter()
                            .filter_map(|&i| current_schema.fields().get(i).cloned())
                            .collect();
                        Arc::new(arrow_schema::Schema::new(fields))
                    }
                    None => current_schema,
                };
                let mut source: Arc<dyn datafusion::datasource::source::DataSource> = Arc::new(
                    crate::bundle::schema_rename_filter::SchemaRenameDataSource::new(
                        inner_source,
                        projected_schema,
                        projected_col_ids,
                    ),
                );
                if !deleted.is_empty() {
                    source = Arc::new(
                        crate::bundle::deleted_row_filter::DeletedRowFilterDataSource::new(
                            source,
                            Arc::new(deleted),
                        ),
                    );
                }
                return Ok(Arc::new(DataSourceExec::new(source)));
            }
        }

        // Phase 3: Full scan with block cache
        let cache_key = self.cache_key();
        let validated = self
            .version_validated
            .load(std::sync::atomic::Ordering::Relaxed);

        // Try to serve from the block cache (stores base data with internal names).
        // Only use cache after version has been validated (first scan reads through reader).
        let mut source: Arc<dyn datafusion::datasource::source::DataSource> = if validated {
            if let Some(cached) = GLOBAL_BLOCK_CACHE.get(&cache_key) {
                log::debug!("Block cache hit for {}", cache_key);
                record_cache_operation("block_cache", true);
                // Use the cached batch's own schema (derived from the reader's actual types)
                let batch_schema = cached
                    .batches
                    .first()
                    .map(|b| b.schema())
                    .unwrap_or_else(|| self.schema.read().clone());
                Arc::new(MemorySourceConfig::try_new(
                    &[cached.batches.as_ref().clone()],
                    batch_schema,
                    projection.cloned(),
                )?)
            } else {
                log::debug!("Block cache miss for {}", cache_key);
                record_cache_operation("block_cache", false);
                // Validated but not cached (evicted or first scan after validation).
                // Read through reader, rename to internal names, cache result.
                // NOTE: collect() is intentional here — we materialize the block to populate
                // the LRU block cache for subsequent queries. Block sizes are bounded by the
                // source row-group size (typically ~128MB), and the cache enforces a global
                // memory budget via eviction.
                let base_source = self.reader.data_source(None, &[], None, None).await?;
                let task_ctx = Arc::new(
                    datafusion::execution::TaskContext::default()
                        .with_runtime(Arc::clone(state.runtime_env())),
                );
                let stream = base_source.open(0, task_ctx)?;
                let batches = Self::collect(self.reader.url(), stream).await?;
                let batches = Self::rename_batches_with_internal_names(batches, &self.column_ids);
                let batch_schema = batches.first().map(|b| b.schema()).unwrap_or_else(|| {
                    log::debug!(
                        "Block {} returned no batches on re-read; using stored schema",
                        self.id
                    );
                    self.schema.read().clone()
                });
                GLOBAL_BLOCK_CACHE.insert(cache_key.clone(), batches.clone());
                Arc::new(MemorySourceConfig::try_new(
                    &[batches],
                    batch_schema,
                    projection.cloned(),
                )?)
            }
        } else {
            // First scan: read through reader (validates version), rename, then cache.
            // NOTE: collect() is intentional — see comment above for rationale.
            record_cache_operation("block_cache", false);
            let base_source = self.reader.data_source(None, &[], None, None).await?;
            let task_ctx = Arc::new(
                datafusion::execution::TaskContext::default()
                    .with_runtime(Arc::clone(state.runtime_env())),
            );
            let stream = base_source.open(0, task_ctx)?;
            let batches = Self::collect(self.reader.url(), stream).await?;
            let batches = Self::rename_batches_with_internal_names(batches, &self.column_ids);
            let batch_schema = batches.first().map(|b| b.schema()).unwrap_or_else(|| {
                log::debug!(
                    "Block {} returned no batches on first scan; using stored schema",
                    self.id
                );
                self.schema.read().clone()
            });

            // Version validated successfully — mark and cache.
            // Update self.schema with actual reader types for consistent planning.
            self.version_validated
                .store(true, std::sync::atomic::Ordering::Relaxed);
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
            source = Arc::new(
                crate::bundle::deleted_row_filter::DeletedRowFilterDataSource::new(
                    source,
                    Arc::new(deleted),
                ),
            );
        }

        // Apply update overlay if there are updates for this block
        if !overlays.is_empty() {
            // Build projected column_ids matching the scan output columns
            let projected_col_ids = match projection {
                Some(proj) => proj
                    .iter()
                    .filter_map(|&i| self.column_ids.get(i).copied())
                    .collect::<Vec<_>>(),
                None => self.column_ids.clone(),
            };
            let current_schema = self.schema.read().clone();
            let projected_schema = match projection {
                Some(proj) => {
                    let fields: Vec<_> = proj
                        .iter()
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

/// Build a DataFusion `Statistics` object from our per-column stats, for the query optimizer.
/// Uses the internal-name schema (col_<id>) to populate column statistics positionally.
fn build_df_statistics(
    col_stats: &[bundlebase_data::ColumnStats],
    schema: &arrow_schema::SchemaRef,
    num_rows: Option<usize>,
) -> datafusion::common::Statistics {
    use datafusion::common::stats::Precision;
    use datafusion::common::{ColumnStatistics, Statistics};

    let column_statistics = schema
        .fields()
        .iter()
        .enumerate()
        .map(|(i, _field)| {
            let cs = match col_stats.get(i) {
                Some(s) => s,
                None => return ColumnStatistics::new_unknown(),
            };
            let min_val = cs
                .min
                .as_ref()
                .and_then(stat_value_to_scalar)
                .map(Precision::Exact)
                .unwrap_or(Precision::Absent);
            let max_val = cs
                .max
                .as_ref()
                .and_then(stat_value_to_scalar)
                .map(Precision::Exact)
                .unwrap_or(Precision::Absent);
            ColumnStatistics {
                null_count: Precision::Exact(cs.null_count as usize),
                max_value: max_val,
                min_value: min_val,
                distinct_count: if cs.distinct_count > 0 {
                    Precision::Inexact(cs.distinct_count as usize)
                } else {
                    Precision::Absent
                },
                ..Default::default()
            }
        })
        .collect();

    Statistics {
        num_rows: num_rows.map(Precision::Exact).unwrap_or(Precision::Absent),
        total_byte_size: Precision::Absent,
        column_statistics,
    }
}

/// Convert a typed `StatValue` to a DataFusion `ScalarValue` for the query optimizer.
fn stat_value_to_scalar(
    sv: &bundlebase_data::StatValue,
) -> Option<datafusion::scalar::ScalarValue> {
    use bundlebase_data::StatValue;
    use datafusion::scalar::ScalarValue;
    match sv {
        StatValue::Int8(n) => Some(ScalarValue::Int8(Some(*n))),
        StatValue::Int16(n) => Some(ScalarValue::Int16(Some(*n))),
        StatValue::Int32(n) => Some(ScalarValue::Int32(Some(*n))),
        StatValue::Int64(n) => Some(ScalarValue::Int64(Some(*n))),
        StatValue::UInt8(n) => Some(ScalarValue::UInt8(Some(*n))),
        StatValue::UInt16(n) => Some(ScalarValue::UInt16(Some(*n))),
        StatValue::UInt32(n) => Some(ScalarValue::UInt32(Some(*n))),
        StatValue::UInt64(n) => Some(ScalarValue::UInt64(Some(*n))),
        StatValue::Float32(f) => Some(ScalarValue::Float32(Some(*f))),
        StatValue::Float64(f) => Some(ScalarValue::Float64(Some(*f))),
        StatValue::Utf8(s) => Some(ScalarValue::Utf8(Some(s.clone()))),
        StatValue::Boolean(b) => Some(ScalarValue::Boolean(Some(*b))),
        StatValue::Date32(n) => Some(ScalarValue::Date32(Some(*n))),
        StatValue::Date64(n) => Some(ScalarValue::Date64(Some(*n))),
        StatValue::TimestampSecond(n) => Some(ScalarValue::TimestampSecond(Some(*n), None)),
        StatValue::TimestampMillisecond(n) => {
            Some(ScalarValue::TimestampMillisecond(Some(*n), None))
        }
        StatValue::TimestampMicrosecond(n) => {
            Some(ScalarValue::TimestampMicrosecond(Some(*n), None))
        }
        StatValue::TimestampNanosecond(n) => Some(ScalarValue::TimestampNanosecond(Some(*n), None)),
        StatValue::Time32Second(n) => Some(ScalarValue::Time32Second(Some(*n))),
        StatValue::Time32Millisecond(n) => Some(ScalarValue::Time32Millisecond(Some(*n))),
        StatValue::Time64Microsecond(n) => Some(ScalarValue::Time64Microsecond(Some(*n))),
        StatValue::Time64Nanosecond(n) => Some(ScalarValue::Time64Nanosecond(Some(*n))),
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
