//! `search()` table function for BM25 full-text search
//!
//! Provides a table function that replaces `FROM bundle` with search results
//! including BM25 scores. Usage:
//!
//! ```sql
//! SELECT Company, City, _score
//! FROM search('my_search', 'company:group AND city:east')
//! ORDER BY _score DESC
//! LIMIT 10
//! ```
//!
//! Also supports single-arg form when only one text index exists:
//! ```sql
//! SELECT * FROM search('query')
//! ```

use crate::bundle::{BundleFacade, Pack};
use crate::data::{BlockId, ObjectId, ObjectIdAlias, RowId};
use crate::index::{IndexDefinition, TextIndex};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use arrow::array::{Float64Array, RecordBatch, UInt64Array};
use arrow::compute;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableFunctionImpl, TableProvider};
use datafusion::common::{project_schema, Statistics};
use datafusion::datasource::source::{DataSource, DataSourceExec};
use datafusion::datasource::TableType;
use datafusion::error::DataFusionError;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::ExecutionPlan;
use futures::stream::{self, StreamExt};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

/// Extract a string literal from an Expr
fn extract_string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(datafusion::common::ScalarValue::Utf8(Some(s)), _) => Some(s.clone()),
        Expr::Literal(datafusion::common::ScalarValue::Utf8View(Some(s)), _) => {
            Some(s.to_string())
        }
        _ => None,
    }
}

/// Table function that creates a `SearchResultTableProvider` for text search
pub struct SearchTableFunction {
    facade: std::sync::Weak<dyn BundleFacade>,
}

impl std::fmt::Debug for SearchTableFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchTableFunction").finish()
    }
}

impl SearchTableFunction {
    pub fn new(facade: std::sync::Weak<dyn BundleFacade>) -> Self {
        Self { facade }
    }
}

impl TableFunctionImpl for SearchTableFunction {
    fn call(&self, args: &[Expr]) -> datafusion::common::Result<Arc<dyn TableProvider>> {
        let facade = self.facade.upgrade().ok_or_else(|| {
            DataFusionError::Internal("Bundle has been dropped".to_string())
        })?;

        let indexes = facade.indexes();

        let (index_name, query) = match args.len() {
            1 => {
                // Single arg: search('query') — find the single text index
                let query = extract_string_literal(&args[0]).ok_or_else(|| {
                    DataFusionError::Plan(
                        "search() argument must be a string literal (query)".to_string(),
                    )
                })?;

                let text_indexes: Vec<_> =
                    indexes.iter().filter(|idx| idx.is_text()).collect();

                match text_indexes.len() {
                    0 => {
                        return Err(DataFusionError::Plan(
                            "search() with 1 argument requires exactly one text index, but none exist on this bundle".to_string(),
                        ));
                    }
                    1 => (text_indexes[0].name().to_string(), query),
                    n => {
                        let names: Vec<String> =
                            text_indexes.iter().map(|idx| idx.name().to_string()).collect();
                        return Err(DataFusionError::Plan(format!(
                            "search() with 1 argument requires exactly one text index, but {} exist: {}. Use search('index_name', 'query') to specify which index.",
                            n,
                            names.join(", ")
                        )));
                    }
                }
            }
            2 => {
                // Two args: search('index_name', 'query')
                let index_name = extract_string_literal(&args[0]).ok_or_else(|| {
                    DataFusionError::Plan(
                        "search() first argument must be a string literal (index name)".to_string(),
                    )
                })?;

                let query = extract_string_literal(&args[1]).ok_or_else(|| {
                    DataFusionError::Plan(
                        "search() second argument must be a string literal (query)".to_string(),
                    )
                })?;

                (index_name, query)
            }
            _ => {
                return Err(DataFusionError::Plan(
                    "search() requires 1 or 2 arguments: search('query') or search('index_name', 'query')".to_string(),
                ));
            }
        };

        // Look up the index definition by name only
        let index_def = indexes
            .iter()
            .find(|idx| idx.is_text() && idx.name() == index_name)
            .ok_or_else(|| {
                let available_text: Vec<String> = indexes
                    .iter()
                    .filter(|idx| idx.is_text())
                    .map(|idx| idx.name().to_string())
                    .collect();

                if available_text.is_empty() {
                    DataFusionError::Plan(format!(
                        "Text index '{}' not found. No text indexes exist on this bundle",
                        index_name
                    ))
                } else {
                    DataFusionError::Plan(format!(
                        "Text index '{}' not found. Available text indexes: {}",
                        index_name,
                        available_text.join(", ")
                    ))
                }
            })?;

        let ctx = facade.ctx();
        let data_dir = facade.data_dir();
        let config = facade.config();
        let packs = facade.packs();

        Ok(Arc::new(SearchResultTableProvider {
            index_name,
            query,
            index_def: index_def.clone(),
            ctx,
            data_dir,
            config,
            packs,
        }))
    }
}

/// Table provider that executes a text search and returns matching rows with scores
struct SearchResultTableProvider {
    index_name: String,
    query: String,
    index_def: Arc<IndexDefinition>,
    ctx: Arc<datafusion::prelude::SessionContext>,
    data_dir: Arc<dyn crate::io::IOReadWriteDir>,
    config: Arc<crate::BundleConfig>,
    packs: HashMap<ObjectId, Arc<Pack>>,
}

impl std::fmt::Debug for SearchResultTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchResultTableProvider")
            .field("index_name", &self.index_name)
            .field("query", &self.query)
            .finish()
    }
}

impl SearchResultTableProvider {
    /// Returns the data schema from the first available block.
    /// All blocks in a bundle are guaranteed to share the same schema,
    /// so any block's schema is representative.
    fn data_schema(&self) -> SchemaRef {
        for pack in self.packs.values() {
            for block in pack.blocks() {
                return block.schema();
            }
        }
        Arc::new(Schema::empty())
    }

    fn output_schema(&self) -> SchemaRef {
        let data_schema = self.data_schema();
        let mut fields: Vec<Arc<Field>> = data_schema.fields().iter().cloned().collect();
        fields.push(Arc::new(Field::new("_score", DataType::Float64, false)));
        Arc::new(Schema::new(fields))
    }
}

#[async_trait]
impl TableProvider for SearchResultTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.output_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> datafusion::common::Result<Vec<TableProviderFilterPushDown>> {
        Ok(filters
            .iter()
            .map(|_| TableProviderFilterPushDown::Inexact)
            .collect())
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        let output_schema = self.output_schema();

        // Step 1: Load text indexes and execute search across all indexed blocks
        let all_indexed_blocks = self.index_def.all_indexed_blocks();

        if all_indexed_blocks.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Build a map from BlockId -> block_ref (u16) preserving the order from index building.
        // During index building, blocks are assigned sequential ObjectIdAlias values (0, 1, 2, ...)
        // based on their position. We must use the same mapping here so that RowId block_refs match.
        // NOTE: If multiple IndexedBlocks entries exist (from incremental indexing), block_refs
        // could collide across entries. This works correctly when all blocks are in a single
        // IndexedBlocks entry (the common case after reindex).
        let mut block_id_to_ref: HashMap<BlockId, u16> = HashMap::new();
        for indexed_blocks in &all_indexed_blocks {
            for (ref_idx, vb) in indexed_blocks.blocks().iter().enumerate() {
                block_id_to_ref.insert(vb.block, ref_idx as u16);
            }
        }

        // Collect (row_id, score) pairs from all indexed blocks
        let mut row_id_scores: Vec<(RowId, f64)> = Vec::new();
        // Default to 10,000 results when no SQL LIMIT is specified.
        // This prevents unbounded result sets for broad search terms.
        let search_limit = limit.unwrap_or(10000);

        for indexed_blocks in &all_indexed_blocks {
            let index_path = indexed_blocks.path();

            let index_file = ObjectStoreFile::from_str(
                index_path,
                self.data_dir.as_ref(),
                self.config.clone(),
            )
            .map_err(|e| DataFusionError::External(e))?;

            let index_bytes = index_file
                .read_bytes()
                .await
                .map_err(|e| DataFusionError::External(e))?
                .ok_or_else(|| {
                    DataFusionError::Execution(format!(
                        "Text index file not found: {}",
                        index_path
                    ))
                })?;

            let text_index = TextIndex::deserialize(index_bytes)
                .map_err(|e| DataFusionError::External(e))?;

            let results = text_index
                .search(&self.query, search_limit)
                .map_err(|e| DataFusionError::External(e))?;

            for result in results {
                row_id_scores.push((result.row_id, result.score as f64));
            }
        }

        if row_id_scores.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Sort by score descending
        row_id_scores.sort_by(|a, b| b.1.total_cmp(&a.1));

        // Build a map from row_id to score for lookup
        let score_map: Arc<HashMap<u64, f64>> = Arc::new(
            row_id_scores
                .iter()
                .map(|(row_id, score)| (row_id.as_u64(), *score))
                .collect(),
        );

        // Pre-build the set of block_refs that have at least one search match.
        // This allows skipping entire blocks with zero matches during streaming.
        let matching_block_refs: Arc<HashSet<u16>> = Arc::new(
            row_id_scores
                .iter()
                .map(|(row_id, _)| row_id.block_ref().as_u16())
                .collect(),
        );

        let projected_schema = project_schema(&output_schema, projection)?;

        let data_source = SearchDataSource {
            output_schema,
            projected_schema,
            projection: projection.cloned(),
            score_map,
            matching_block_refs,
            block_id_to_ref: Arc::new(block_id_to_ref),
            packs: self.packs.clone(),
            ctx: self.ctx.clone(),
            fetch: limit,
        };

        Ok(Arc::new(DataSourceExec::new(Arc::new(data_source))))
    }
}

impl SearchResultTableProvider {
    fn empty_exec(
        &self,
        schema: &SchemaRef,
        projection: Option<&Vec<usize>>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        let projected_schema = project_schema(schema, projection)?;
        let data_source = EmptySearchDataSource {
            projected_schema,
        };
        Ok(Arc::new(DataSourceExec::new(Arc::new(data_source))))
    }
}

/// DataSource that streams search results by scanning blocks on demand.
///
/// The Tantivy search phase (producing `score_map`) completes in `scan()`,
/// but the data-fetching phase is deferred to `open()` so rows are streamed
/// rather than materialized into a `Vec<RecordBatch>`.
struct SearchDataSource {
    output_schema: SchemaRef,
    projected_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    score_map: Arc<HashMap<u64, f64>>,
    /// Block refs (u16) that have at least one matching row — used to skip non-matching blocks.
    matching_block_refs: Arc<HashSet<u16>>,
    block_id_to_ref: Arc<HashMap<BlockId, u16>>,
    packs: HashMap<ObjectId, Arc<Pack>>,
    ctx: Arc<datafusion::prelude::SessionContext>,
    fetch: Option<usize>,
}

impl fmt::Debug for SearchDataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchDataSource")
            .field("matches", &self.score_map.len())
            .field("fetch", &self.fetch)
            .finish()
    }
}

impl fmt::Display for SearchDataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SearchDataSource[matches={}, fetch={:?}]",
            self.score_map.len(),
            self.fetch
        )
    }
}

impl DataSource for SearchDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let output_schema = self.output_schema.clone();
        let projected_schema = self.projected_schema.clone();
        let score_map = self.score_map.clone();
        let block_id_to_ref = self.block_id_to_ref.clone();
        let ctx = self.ctx.clone();
        let projection = self.projection.clone();

        let matching_block_refs = self.matching_block_refs.clone();

        // Collect (block, block_ref_idx) pairs for blocks that are in the index
        // and have at least one matching row — skip blocks with zero search matches.
        let mut blocks_to_scan: Vec<(Arc<crate::bundle::DataBlock>, u16)> = Vec::new();
        for pack in self.packs.values() {
            for block in pack.blocks() {
                if let Some(&ref_idx) = block_id_to_ref.get(block.id()) {
                    if matching_block_refs.contains(&ref_idx) {
                        blocks_to_scan.push((block, ref_idx));
                    }
                }
            }
        }

        // Build an async stream that iterates blocks, extracts rowids, filters, and appends scores
        let stream = stream::iter(blocks_to_scan)
            .then(move |(block, block_ref_idx)| {
                let score_map = score_map.clone();
                let output_schema = output_schema.clone();
                let ctx = ctx.clone();
                let projection = projection.clone();

                async move {
                    let reader = block.reader();
                    let block_ref = ObjectIdAlias::from(block_ref_idx);

                    let rowid_stream = reader
                        .extract_rowids_stream(block_ref, ctx, None)
                        .await
                        .map_err(|e| DataFusionError::External(e))?;

                    // Return a stream of filtered+scored batches for this block
                    let batch_stream = rowid_stream.filter_map(move |batch_result| {
                        let score_map = score_map.clone();
                        let output_schema = output_schema.clone();
                        let projection = projection.clone();

                        async move {
                            let rowid_batch = match batch_result {
                                Ok(b) => b,
                                Err(e) => return Some(Err(DataFusionError::External(e))),
                            };

                            let batch = &rowid_batch.batch;
                            let row_ids = &rowid_batch.row_ids;

                            // Find matching rows in this batch
                            let mut matching_indices: Vec<usize> = Vec::new();
                            let mut matching_scores: Vec<f64> = Vec::new();

                            for (idx, row_id) in row_ids.iter().enumerate() {
                                if let Some(&score) = score_map.get(&row_id.as_u64()) {
                                    matching_indices.push(idx);
                                    matching_scores.push(score);
                                }
                            }

                            if matching_indices.is_empty() {
                                return None;
                            }

                            // Filter the batch to only matching rows using take
                            let indices = UInt64Array::from(
                                matching_indices
                                    .iter()
                                    .map(|&i| i as u64)
                                    .collect::<Vec<_>>(),
                            );
                            let mut filtered_columns: Vec<Arc<dyn arrow::array::Array>> =
                                Vec::new();
                            for col in batch.columns() {
                                match compute::take(col, &indices, None) {
                                    Ok(filtered) => filtered_columns.push(filtered),
                                    Err(e) => {
                                        return Some(Err(DataFusionError::ArrowError(
                                            Box::new(e),
                                            None,
                                        )))
                                    }
                                }
                            }

                            // Append score column
                            let score_array = Float64Array::from(matching_scores);
                            filtered_columns.push(Arc::new(score_array));

                            let result_batch =
                                match RecordBatch::try_new(output_schema.clone(), filtered_columns)
                                {
                                    Ok(b) => b,
                                    Err(e) => {
                                        return Some(Err(DataFusionError::ArrowError(
                                            Box::new(e),
                                            None,
                                        )))
                                    }
                                };

                            // Apply projection if specified
                            let final_batch = if let Some(ref proj) = projection {
                                let projected_columns: Vec<_> =
                                    proj.iter().map(|&i| result_batch.column(i).clone()).collect();
                                let proj_schema = Arc::new(
                                    match output_schema.project(proj) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            return Some(Err(DataFusionError::ArrowError(
                                                Box::new(e),
                                                None,
                                            )))
                                        }
                                    },
                                );
                                match RecordBatch::try_new(proj_schema, projected_columns) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        return Some(Err(DataFusionError::ArrowError(
                                            Box::new(e),
                                            None,
                                        )))
                                    }
                                }
                            } else {
                                result_batch
                            };

                            Some(Ok(final_batch))
                        }
                    });

                    Ok::<_, DataFusionError>(batch_stream)
                }
            })
            // Flatten per-block streams into a single stream of RecordBatch results
            .flat_map(|block_stream_result| {
                match block_stream_result {
                    Ok(batch_stream) => {
                        futures::future::Either::Left(batch_stream)
                    }
                    Err(e) => {
                        futures::future::Either::Right(stream::once(async move { Err(e) }))
                    }
                }
            });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            projected_schema,
            stream,
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: datafusion::physical_plan::DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SearchDataSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new(self.projected_schema.clone())
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        Ok(Statistics::new_unknown(&self.output_schema))
    }

    fn with_fetch(&self, _limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        None
    }

    fn fetch(&self) -> Option<usize> {
        self.fetch
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        Ok(None)
    }
}

/// Simple DataSource that returns an empty stream for no-match cases
struct EmptySearchDataSource {
    projected_schema: SchemaRef,
}

impl fmt::Debug for EmptySearchDataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmptySearchDataSource").finish()
    }
}

impl fmt::Display for EmptySearchDataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EmptySearchDataSource")
    }
}

impl DataSource for EmptySearchDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let schema = self.projected_schema.clone();
        let empty: futures::stream::Empty<datafusion::common::Result<RecordBatch>> =
            stream::empty();
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, empty)))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: datafusion::physical_plan::DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EmptySearchDataSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new(self.projected_schema.clone())
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        Ok(Statistics::new_unknown(&self.projected_schema))
    }

    fn with_fetch(&self, _limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        None
    }

    fn fetch(&self) -> Option<usize> {
        None
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        Ok(None)
    }
}
