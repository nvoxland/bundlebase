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

use crate::bundle::column_metadata;
use crate::bundle::{AnyOperation, BundleFacade, Operation, Pack};
use crate::data::{BlockId, ObjectId, ObjectIdAlias, RowId};
use crate::index::TextIndex;
use crate::index::IndexDefinition;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use arrow::array::{Float64Array, RecordBatch, UInt64Array};
use arrow::compute;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableFunctionImpl, TableProvider};
use datafusion::common::{project_schema, Statistics};
use datafusion::datasource::source::{DataSource, DataSourceExec};
use datafusion::datasource::MemTable;
use datafusion::datasource::TableType;
use datafusion::error::DataFusionError;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::{SessionConfig, SessionContext};
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
            DataFusionError::Internal("Bundle has been dropped (while calling search() table function)".to_string())
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
            operations: facade.operations(),
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
    operations: Vec<AnyOperation>,
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
    /// Returns the unified physical schema derived from all AttachBlock operations,
    /// plus the corresponding column IDs.
    fn physical_schema_and_ids(&self) -> (SchemaRef, Vec<crate::object_id::ColumnId>) {
        column_metadata::unified_physical_schema(&self.operations)
    }

    /// Returns the physical schema plus `_score` column.
    /// This is the schema of raw search result batches before operations are applied.
    fn physical_output_schema(&self) -> SchemaRef {
        let (physical, _) = self.physical_schema_and_ids();
        let mut fields: Vec<Arc<Field>> = physical.fields().iter().cloned().collect();
        fields.push(Arc::new(Field::new("_score", DataType::Float64, false)));
        Arc::new(Schema::new(fields))
    }

    /// Compute the logical output schema by applying operations to an empty DataFrame.
    /// This reuses the authoritative `apply_dataframe` code path, avoiding duplication
    /// of rename/drop/cast/add logic.
    fn output_schema(&self) -> SchemaRef {
        let physical_with_score = self.physical_output_schema();

        let result = futures::executor::block_on(async {
            let mut config = SessionConfig::new();
            config.options_mut().sql_parser.enable_ident_normalization = false;
            let ctx = SessionContext::new_with_config(config);
            let empty_batch = RecordBatch::new_empty(physical_with_score.clone());
            ctx.register_batch("bundle", empty_batch)
                .map_err(|e| DataFusionError::External(Box::new(e)))?;
            let mut df = ctx.table("bundle").await?;

            let mut col_names = column_metadata::initial_column_names(&self.operations);
            for op in self.operations.iter() {
                df = op
                    .apply_dataframe(df, ctx.clone().into(), &mut col_names)
                    .await
                    .map_err(|e| DataFusionError::External(e))?;
            }
            Ok::<_, DataFusionError>(Arc::new(df.schema().as_arrow().clone()))
        });

        result.unwrap_or_else(|e| {
            log::warn!("Failed to compute output schema via operations, falling back to physical schema: {}", e);
            physical_with_score
        })
    }

    /// Rewrite logical field names in a search query to physical field names.
    /// E.g., after renaming "Answer" → "answer", rewrites "answer:foo" → "Answer:foo"
    fn rewrite_query_fields(&self, query: &str) -> String {
        let index_column_ids = self.index_def.column_ids();

        let id_to_current = column_metadata::resolved_column_names(&self.operations);
        let id_to_original = column_metadata::initial_column_names(&self.operations);

        let mut logical_to_physical: HashMap<String, String> = HashMap::new();
        for col_id in index_column_ids.iter() {
            if let (Some(current_name), Some(original_name)) = (
                id_to_current.get(col_id),
                id_to_original.get(col_id),
            ) {
                if current_name != original_name {
                    logical_to_physical.insert(current_name.clone(), original_name.clone());
                }
            }
        }

        if logical_to_physical.is_empty() {
            return query.to_string();
        }

        let mut result = query.to_string();
        for (logical, physical) in &logical_to_physical {
            result = result.replace(
                &format!("{}:", logical),
                &format!("{}:", physical),
            );
        }
        result
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
        let rewritten_query = self.rewrite_query_fields(&self.query);

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
                .search(&rewritten_query, search_limit)
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
            physical_output_schema: self.physical_output_schema(),
            projected_schema,
            projection: projection.cloned(),
            score_map,
            matching_block_refs,
            block_id_to_ref: Arc::new(block_id_to_ref),
            packs: self.packs.clone(),
            ctx: self.ctx.clone(),
            operations: self.operations.clone(),
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
    /// The final logical schema (operations applied + `_score`).
    output_schema: SchemaRef,
    /// The raw physical schema (block columns + `_score`), before operations.
    physical_output_schema: SchemaRef,
    projected_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    score_map: Arc<HashMap<u64, f64>>,
    /// Block refs (u16) that have at least one matching row — used to skip non-matching blocks.
    matching_block_refs: Arc<HashSet<u16>>,
    block_id_to_ref: Arc<HashMap<BlockId, u16>>,
    packs: HashMap<ObjectId, Arc<Pack>>,
    ctx: Arc<datafusion::prelude::SessionContext>,
    /// Operations to apply to raw search results to produce logical output.
    operations: Vec<AnyOperation>,
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
        let physical_output_schema = self.physical_output_schema.clone();
        let projected_schema = self.projected_schema.clone();
        let score_map = self.score_map.clone();
        let block_id_to_ref = self.block_id_to_ref.clone();
        let ctx = self.ctx.clone();
        let projection = self.projection.clone();
        let operations = self.operations.clone();

        let matching_block_refs = self.matching_block_refs.clone();

        // Collect (block, block_ref_idx) for blocks that are in the index
        // and have at least one matching row — skip blocks with zero search matches.
        let mut blocks_to_scan: Vec<(Arc<crate::bundle::DataBlock>, u16)> = Vec::new();
        for pack in self.packs.values() {
            for block in pack.blocks() {
                if let Some(&ref_idx) = block_id_to_ref.get(block.id()) {
                    if matching_block_refs.contains(&ref_idx) {
                        blocks_to_scan.push((block.clone(), ref_idx));
                    }
                }
            }
        }

        // Check if any operations transform data (not just rename/drop schema changes)
        let has_data_transforming_ops = operations.iter().any(|op| {
            matches!(
                op,
                AnyOperation::AddColumn(_)
                    | AnyOperation::CastColumn(_)
                    | AnyOperation::Filter(_)
            )
        });

        // Build an async stream that collects raw batches, applies operations, then streams results
        let result_stream = stream::once(async move {
            // Step 1: Collect raw physical batches with scores from all matching blocks
            let mut raw_batches: Vec<RecordBatch> = Vec::new();

            // Build a mapping from ColumnId → position in unified schema.
            // With shared ColumnIds across blocks, we can align by ID directly.
            let (_, unified_column_ids) = column_metadata::unified_physical_schema(&operations);
            let num_physical_cols = physical_output_schema.fields().len() - 1;
            let unified_id_to_pos: HashMap<crate::object_id::ColumnId, usize> = unified_column_ids
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i))
                .collect();

            for (block, block_ref_idx) in blocks_to_scan {
                let reader = block.reader();
                let block_ref = ObjectIdAlias::from(block_ref_idx);

                // Build mapping: unified position → block column index (None if block lacks this column)
                let mut unified_to_block: Vec<Option<usize>> = vec![None; num_physical_cols];
                for (block_idx, col_id) in block.column_ids().iter().enumerate() {
                    if let Some(&unified_pos) = unified_id_to_pos.get(col_id) {
                        unified_to_block[unified_pos] = Some(block_idx);
                    }
                }

                let mut rowid_stream = reader
                    .extract_rowids_stream(block_ref, ctx.clone(), None)
                    .await
                    .map_err(|e| DataFusionError::External(e))?;

                while let Some(batch_result) = rowid_stream.next().await {
                    let rowid_batch = batch_result
                        .map_err(|e| DataFusionError::External(e))?;

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
                        continue;
                    }

                    let num_matching = matching_indices.len();

                    // Filter the batch to only matching rows using take
                    let indices = UInt64Array::from(
                        matching_indices
                            .iter()
                            .map(|&i| i as u64)
                            .collect::<Vec<_>>(),
                    );

                    // Align columns to unified schema order, inserting nulls for missing columns
                    let mut aligned_columns: Vec<Arc<dyn arrow::array::Array>> = Vec::new();
                    for (unified_pos, maybe_block_idx) in unified_to_block.iter().enumerate() {
                        match maybe_block_idx {
                            Some(block_idx) => {
                                let filtered = compute::take(&batch.columns()[*block_idx], &indices, None)
                                    .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
                                aligned_columns.push(filtered);
                            }
                            None => {
                                // Block doesn't have this column — insert null array of correct type
                                let field = physical_output_schema.field(unified_pos);
                                let null_array = arrow::array::new_null_array(field.data_type(), num_matching);
                                aligned_columns.push(null_array);
                            }
                        }
                    }

                    // Append score column
                    let score_array = Float64Array::from(matching_scores);
                    aligned_columns.push(Arc::new(score_array));

                    let result_batch = RecordBatch::try_new(
                        physical_output_schema.clone(),
                        aligned_columns,
                    )
                    .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;

                    raw_batches.push(result_batch);
                }
            }

            if raw_batches.is_empty() {
                // Return empty batch with the correct output schema
                return Ok(stream::iter(Vec::<datafusion::common::Result<RecordBatch>>::new()));
            }

            // Step 2: Apply operations to transform raw batches into logical output
            let final_batches = if has_data_transforming_ops {
                // Register raw batches as a "bundle" table and apply operations
                let mut config = SessionConfig::new();
                config.options_mut().sql_parser.enable_ident_normalization = false;
                let op_ctx = SessionContext::new_with_config_rt(config, ctx.runtime_env());

                let mem_table = MemTable::try_new(physical_output_schema.clone(), vec![raw_batches])?;
                op_ctx.register_table("bundle", Arc::new(mem_table))?;
                let mut df = op_ctx.table("bundle").await?;

                // Apply each operation to transform the data
                let mut col_names = column_metadata::initial_column_names(&operations);
                for op in operations.iter() {
                    df = op
                        .apply_dataframe(df, op_ctx.clone().into(), &mut col_names)
                        .await
                        .map_err(|e: crate::BundlebaseError| {
                            DataFusionError::External(e)
                        })?;
                }

                // Re-register the transformed DataFrame as "bundle" to get _score back
                // Operations only act on bundle columns, so _score passes through
                let result_batches: Vec<RecordBatch> = df
                    .collect()
                    .await?;

                result_batches
            } else {
                // No data-transforming operations, just apply schema rename/drop
                // by re-mapping the physical batches to use the logical schema
                let mut result_batches = Vec::new();
                for batch in raw_batches {
                    let result_batch = RecordBatch::try_new(
                        output_schema.clone(),
                        batch.columns().to_vec(),
                    )
                    .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
                    result_batches.push(result_batch);
                }
                result_batches
            };

            // Step 3: Apply projection if specified
            let projected_batches: Vec<datafusion::common::Result<RecordBatch>> = final_batches
                .into_iter()
                .map(|batch| {
                    if let Some(ref proj) = projection {
                        let projected_columns: Vec<_> =
                            proj.iter().map(|&i| batch.column(i).clone()).collect();
                        let proj_schema = Arc::new(
                            output_schema
                                .project(proj)
                                .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?,
                        );
                        RecordBatch::try_new(proj_schema, projected_columns)
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None).into())
                    } else {
                        Ok(batch)
                    }
                })
                .collect();

            Ok(stream::iter(projected_batches))
        })
        .flat_map(|result| match result {
            Ok(batch_stream) => futures::future::Either::Left(batch_stream),
            Err(e) => futures::future::Either::Right(stream::once(async move { Err(e) })),
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            projected_schema,
            result_stream,
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
        Ok(Statistics::new_unknown(&self.projected_schema))
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
