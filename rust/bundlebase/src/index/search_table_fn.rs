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

use crate::bundle::bundle_schema;
use crate::bundle::{resolve_cast_ops, AnyOperation, BundleFacade, Operation, Pack};
use crate::data::{BlockId, ObjectId, ObjectIdAlias, RowId};
use crate::index::IndexDefinition;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use arrow::array::{Float64Array, RecordBatch, UInt64Array};
use arrow::compute;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::streaming::StreamingTable;
use datafusion::catalog::{Session, TableFunctionImpl, TableProvider};
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{project_schema, Statistics};
use datafusion::datasource::source::{DataSource, DataSourceExec};
use datafusion::datasource::TableType;
use datafusion::error::DataFusionError;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::streaming::PartitionStream;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::prelude::{SessionConfig, SessionContext};
use futures::stream::{self, StreamExt};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

/// Extract a string literal from an Expr
fn extract_string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(datafusion::common::ScalarValue::Utf8(Some(s)), _) => Some(s.clone()),
        Expr::Literal(datafusion::common::ScalarValue::Utf8View(Some(s)), _) => Some(s.to_string()),
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
            DataFusionError::Internal(
                "Bundle has been dropped (while calling search() table function)".to_string(),
            )
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

                let text_indexes: Vec<_> = indexes.iter().filter(|idx| idx.is_inverted()).collect();

                match text_indexes.len() {
                    0 => {
                        return Err(DataFusionError::Plan(
                            "search() with 1 argument requires exactly one text index, but none exist on this bundle".to_string(),
                        ));
                    }
                    1 => (text_indexes[0].name().to_string(), query),
                    n => {
                        let names: Vec<String> = text_indexes
                            .iter()
                            .map(|idx| idx.name().to_string())
                            .collect();
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
            .find(|idx| idx.is_inverted() && idx.name() == index_name)
            .ok_or_else(|| {
                let available_text: Vec<String> = indexes
                    .iter()
                    .filter(|idx| idx.is_inverted())
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
        let bundle_schema = facade.bundle_schema();
        let operations = facade.operations();

        // Precompute the schemas ONCE here, at construction, rather than
        // lazily in the sync `schema()` getter. `TableFunctionImpl::call`
        // runs synchronously on a tokio worker during query planning, so the
        // schema computation (which drives an async DataFusion plan) is run on
        // a dedicated runtime via `run_async_blocking` instead of
        // `futures::executor::block_on`, which would otherwise drive those
        // futures directly on — and stall — the runtime worker.
        //
        // Any schema-computation error is propagated out of `call()` so a bad
        // operation surfaces as a planning error instead of being silently
        // swallowed by a fallback to the wrong (physical) schema.
        let physical_output_schema = physical_output_schema(&bundle_schema);
        let output_schema = run_async_blocking(compute_logical_output_schema(
            physical_output_schema.clone(),
            operations.clone(),
            ctx.clone(),
        ))?;

        Ok(Arc::new(SearchResultTableProvider {
            index_name,
            query,
            index_def: index_def.clone(),
            ctx,
            data_dir,
            config,
            packs,
            bundle_schema,
            operations,
            physical_output_schema,
            output_schema,
        }))
    }
}

/// Returns the physical schema plus `_score` column.
/// This is the schema of raw search result batches before operations are applied.
fn physical_output_schema(bundle_schema: &bundle_schema::BundleSchema) -> SchemaRef {
    let physical = bundle_schema.physical_schema();
    let mut fields: Vec<Arc<Field>> = physical.fields().iter().cloned().collect();
    fields.push(Arc::new(Field::new("_score", DataType::Float64, false)));
    Arc::new(Schema::new(fields))
}

/// Compute the logical output schema by applying operations to an empty
/// DataFrame. This reuses the authoritative `apply_dataframe` code path,
/// avoiding duplication of rename/drop/cast/add logic. Errors are returned so
/// callers can surface them instead of falling back to the physical schema.
async fn compute_logical_output_schema(
    physical_with_score: SchemaRef,
    operations: Vec<AnyOperation>,
    source_ctx: Arc<SessionContext>,
) -> Result<SchemaRef, DataFusionError> {
    let mut config = SessionConfig::new();
    config.options_mut().sql_parser.enable_ident_normalization = false;
    let ctx = SessionContext::new_with_config(config);
    // Copy the bundle's registered UDFs so an operation whose `apply_dataframe`
    // plans SQL referencing a custom function (e.g. a filter/add_column using a
    // registered UDF) can be planned on this throwaway context too — otherwise
    // schema computation errors with "function not found" (bb-nt8).
    {
        let state = source_ctx.state();
        for udf in state.scalar_functions().values() {
            ctx.register_udf(udf.as_ref().clone());
        }
        for udaf in state.aggregate_functions().values() {
            ctx.register_udaf(udaf.as_ref().clone());
        }
        for udwf in state.window_functions().values() {
            ctx.register_udwf(udwf.as_ref().clone());
        }
    }
    let empty_batch = RecordBatch::new_empty(physical_with_score.clone());
    ctx.register_batch("bundle", empty_batch)
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let mut df = ctx.table("bundle").await?;

    let mut bundle_schema = bundle_schema::BundleSchema::initial(&operations);
    let cast_active = resolve_cast_ops(&operations);
    for (op, active) in operations.iter().zip(cast_active.iter()) {
        if *active {
            df = op
                .apply_dataframe(df, ctx.clone().into(), &mut bundle_schema)
                .await
                .map_err(DataFusionError::External)?;
        }
    }

    // Final rename: internal names → user-visible names
    df = bundle_schema
        .rename_to_real_names(df)
        .map_err(DataFusionError::External)?;

    Ok(Arc::new(df.schema().as_arrow().clone()))
}

/// Run an async future to completion from synchronous code without driving it
/// on the calling tokio worker.
///
/// `TableFunctionImpl::call` is a synchronous DataFusion callback that is
/// invoked on a tokio runtime worker during query planning. Driving an async
/// DataFusion plan there with `futures::executor::block_on` blocks that worker
/// (and can deadlock when the plan awaits runtime I/O). Instead we run the
/// future on a dedicated single-threaded runtime on a separate thread and
/// block the (one) calling thread until it completes — the runtime worker pool
/// keeps making progress on other tasks.
fn run_async_blocking<F, T>(fut: F) -> Result<T, DataFusionError>
where
    F: std::future::Future<Output = Result<T, DataFusionError>> + Send + 'static,
    T: Send + 'static,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| {
                        DataFusionError::Execution(format!(
                            "Failed to build runtime for schema computation: {}",
                            e
                        ))
                    })?;
                rt.block_on(fut)
            })
            .join()
            .map_err(|_| {
                DataFusionError::Execution("Schema computation thread panicked".to_string())
            })?
    })
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
    bundle_schema: bundle_schema::BundleSchema,
    operations: Vec<AnyOperation>,
    /// Raw physical schema (block columns + `_score`), before operations.
    /// Precomputed at construction.
    physical_output_schema: SchemaRef,
    /// Logical (user-visible) output schema (operations applied + `_score`).
    /// Precomputed at construction so the sync `schema()` getter never blocks
    /// the runtime worker driving an async plan.
    output_schema: SchemaRef,
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
    /// Rewrite user-visible field names in a search query to internal names.
    /// The text index stores fields using stable internal names, so queries like
    /// "Company:group" need to be rewritten to "col_<hex>:group".
    fn rewrite_query_fields(&self, query: &str) -> String {
        let index_column_ids = self.index_def.column_ids();

        // Map user-visible name → internal name for indexed columns
        let mut user_to_internal_name: HashMap<String, String> = HashMap::new();
        for col_id in index_column_ids.iter() {
            if let Some(current_name) = self.bundle_schema.get(col_id) {
                let internal_name = self
                    .bundle_schema
                    .internal_name(col_id)
                    .expect("column ID from schema");
                user_to_internal_name.insert(current_name.clone(), internal_name);
            }
        }

        if user_to_internal_name.is_empty() {
            return query.to_string();
        }

        let mut result = query.to_string();
        for (user_name, internal_name) in &user_to_internal_name {
            result = result.replace(&format!("{}:", user_name), &format!("{}:", internal_name));
        }
        result
    }
}

#[async_trait]
impl TableProvider for SearchResultTableProvider {
    fn schema(&self) -> SchemaRef {
        self.output_schema.clone()
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
        let output_schema = self.output_schema.clone();
        let rewritten_query = self.rewrite_query_fields(&self.query);

        // Step 1: Identify which IndexedBlocks entry covers each currently-active
        // block, build a unified block_ref namespace, and translate per-entry
        // local block_refs into it.
        //
        // Each IndexedBlocks entry was built independently and assigns
        // local block_ref values 0..N over its own `blocks()` list. With
        // multiple entries (the normal state after the auto-reindex hook),
        // those local refs collide across entries — an entry-#1 hit at
        // (local_ref=0, row=5) packs to the same RowId.as_u64() as an
        // entry-#2 hit at (local_ref=0, row=5). Translating into a unified
        // namespace via RowId::with_block_ref makes score_map keys
        // unambiguous and lets the streaming side in `open()` look hits up
        // by the unified ref it's already using.
        let all_indexed_blocks = self.index_def.all_indexed_blocks();
        if all_indexed_blocks.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Active (block_id, version) set for any column this index covers.
        // A multi-column index needs a block to be active for ALL its
        // columns; using the first column is sufficient because the index
        // is only built when every indexed column is present in the block.
        let first_col_id = self.index_def.column_ids().first().ok_or_else(|| {
            DataFusionError::Internal("Index definition has no columns".to_string())
        })?;
        let active_blocks: Vec<(BlockId, String)> =
            self.bundle_schema.blocks_for_column(first_col_id);

        // For each (block_id, version), find the IndexedBlocks entry that
        // covers it AND remember the entry's local_ref for that block.
        //
        // We walk entries newest-first so that if a block was reindexed
        // (e.g. after REPLACE), we pick the most recent entry's coverage.
        struct PerEntry {
            indexed_blocks: Arc<bundlebase_common::IndexedBlocks>,
            /// local_ref (position in entry.blocks()) → unified_ref (u16)
            local_to_unified: HashMap<u16, u16>,
        }
        let mut per_entry: Vec<PerEntry> = Vec::new();
        let mut entry_idx_by_path: HashMap<String, usize> = HashMap::new();
        let mut block_id_to_ref: HashMap<BlockId, u16> = HashMap::new();
        let mut next_unified_ref: Option<u16> = Some(0);

        for (block_id, version) in &active_blocks {
            let vb = bundlebase_common::VersionedBlockId::new(*block_id, version.clone());
            let Some(entry) = self.index_def.indexed_blocks(&vb) else {
                // Block isn't covered by any IndexedBlocks entry — fall
                // through; the streaming side won't include it because no
                // unified_ref is assigned, and a query that needs it will
                // simply miss it (matching today's behaviour for unindexed
                // blocks).
                continue;
            };
            let blocks = entry.blocks();
            let local_pos = blocks
                .iter()
                .position(|b| &b.block == block_id && &b.version == version)
                .ok_or_else(|| {
                    DataFusionError::Internal(format!(
                        "indexed_blocks() returned an entry that does not contain block {:?}@{}",
                        block_id, version
                    ))
                })?;
            let local_ref = u16::try_from(local_pos).map_err(|_| {
                DataFusionError::Execution(format!(
                    "Index entry covers more than {} blocks; block {} is out of range",
                    u16::MAX,
                    block_id
                ))
            })?;

            // Assign a fresh unified_ref to this block (each unique BlockId
            // gets exactly one, regardless of how many entries cover it).
            let unified_ref = match block_id_to_ref.get(block_id) {
                Some(r) => *r,
                None => {
                    let r = next_unified_ref.ok_or_else(|| {
                        DataFusionError::Execution(format!(
                            "search() supports at most {} active blocks per index",
                            u16::MAX
                        ))
                    })?;
                    next_unified_ref = r.checked_add(1);
                    block_id_to_ref.insert(*block_id, r);
                    r
                }
            };

            let path = entry.path().to_string();
            let idx = *entry_idx_by_path.entry(path).or_insert_with(|| {
                per_entry.push(PerEntry {
                    indexed_blocks: entry.clone(),
                    local_to_unified: HashMap::new(),
                });
                per_entry.len() - 1
            });
            per_entry[idx]
                .local_to_unified
                .insert(local_ref, unified_ref);
        }

        if per_entry.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Step 2: Load each pass's serialized tar bytes and run a SINGLE
        // unified BM25 query across them all (corpus stats — term DF,
        // total docs, avg doc length — computed across the union, not
        // per-pass). Entries whose every block has been superseded simply
        // aren't loaded.
        let mut row_id_scores: Vec<(RowId, f64)> = Vec::new();
        // Default to 10,000 results when no SQL LIMIT is specified. This
        // prevents unbounded result sets for broad search terms.
        let search_limit = limit.unwrap_or(10000);

        // Path-only cache key (paths are content-addressed, so identity
        // implies bytes-identity). On cache hit we skip the I/O entirely;
        // on miss we pay tar reads + assembly once per (set-of-passes)
        // and reuse the unified index for every subsequent search.
        let pass_paths: Vec<String> = per_entry
            .iter()
            .map(|e| e.indexed_blocks.path().to_string())
            .collect();

        // Read pass bytes only on cache miss. We don't know that yet, but
        // the cached path is checked first inside search_unified_cached.
        let mut pass_bytes: Vec<bytes::Bytes> = Vec::with_capacity(per_entry.len());
        if !bundlebase_index::unified_index_cached(&pass_paths) {
            for entry in &per_entry {
                let index_path = entry.indexed_blocks.path();
                let index_file = ObjectStoreFile::from_str(
                    index_path,
                    self.data_dir.as_ref(),
                    self.config.clone(),
                )
                .map_err(DataFusionError::External)?;
                let index_bytes = index_file
                    .read_bytes()
                    .await
                    .map_err(DataFusionError::External)?
                    .ok_or_else(|| {
                        DataFusionError::Execution(format!(
                            "Text index file not found: {}",
                            index_path
                        ))
                    })?;
                pass_bytes.push(index_bytes);
            }
        }

        let hits = bundlebase_index::search_unified_cached(
            &pass_paths,
            &pass_bytes,
            &rewritten_query,
            search_limit,
        )
        .map_err(DataFusionError::External)?;

        for hit in hits {
            // hit.pass_idx indexes into per_entry, so we know exactly which
            // IndexedBlocks (and therefore which local_to_unified map) the
            // hit came from — no segment_id ↔ entry plumbing in the caller.
            let entry = &per_entry[hit.pass_idx];
            let local_ref = hit.row_id.block_ref().as_u16();
            if let Some(&unified_ref) = entry.local_to_unified.get(&local_ref) {
                let unified_row_id = hit.row_id.with_block_ref(ObjectIdAlias::from(unified_ref));
                row_id_scores.push((unified_row_id, hit.score as f64));
            }
            // Hits whose block is covered here at a version no longer
            // active are silently dropped: their local_ref isn't in
            // local_to_unified.
        }

        // Step 3: UPDATEd indexed text. The inverted index holds the BASE
        // values, so for rows whose indexed column(s) carry an update overlay
        // its hits are stale: drop them, then re-evaluate the query against
        // those rows' CURRENT text with a small throwaway index.
        let updated = self.rows_with_updated_index_text(&block_id_to_ref)?;
        if !updated.is_empty() {
            row_id_scores.retain(|(row_id, _)| {
                !updated
                    .get(&row_id.block_ref().as_u16())
                    .is_some_and(|(_, rows)| rows.contains(&row_id.row_number()))
            });
            row_id_scores.extend(
                self.search_updated_rows(&updated, &rewritten_query, search_limit)
                    .await?,
            );
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
            physical_output_schema: self.physical_output_schema.clone(),
            projected_schema,
            projection: projection.cloned(),
            score_map,
            matching_block_refs,
            block_id_to_ref: Arc::new(block_id_to_ref),
            packs: self.packs.clone(),
            ctx: self.ctx.clone(),
            bundle_schema: self.bundle_schema.clone(),
            operations: self.operations.clone(),
            fetch: limit,
        };

        Ok(Arc::new(DataSourceExec::new(Arc::new(data_source))))
    }
}

/// Blocks (by unified ref) whose indexed text was UPDATEd, with the row
/// numbers whose indexed column(s) carry an overlay value.
type UpdatedIndexRows = HashMap<u16, (Arc<crate::bundle::DataBlock>, HashSet<u32>)>;

impl SearchResultTableProvider {
    /// Find rows whose indexed column(s) have an update overlay, per indexed
    /// block. Only overlays that SET an indexed column matter — updates to
    /// other columns don't change what the index should match.
    ///
    /// Limitation: a text index over a COMPUTED column (whose inputs were
    /// updated) isn't covered — its ColumnId never appears in an overlay.
    fn rows_with_updated_index_text(
        &self,
        block_id_to_ref: &HashMap<BlockId, u16>,
    ) -> datafusion::common::Result<UpdatedIndexRows> {
        let index_cols = self.index_def.column_ids();
        let mut updated: UpdatedIndexRows = HashMap::new();
        for pack in self.packs.values() {
            for block in pack.blocks() {
                let Some(&unified_ref) = block_id_to_ref.get(block.id()) else {
                    continue;
                };
                let overlays = block.update_overlays();
                if overlays.is_empty() {
                    continue;
                }
                let merged = crate::bundle::update_overlay::UpdateOverlay::merge(&overlays)
                    .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
                let mut rows = HashSet::new();
                for col_id in index_cols {
                    if let Some((_, is_set)) = merged.columns.get(col_id) {
                        for (pos, row_number) in merged.row_numbers.iter().enumerate() {
                            if is_set.value(pos) {
                                rows.insert(*row_number);
                            }
                        }
                    }
                }
                if !rows.is_empty() {
                    updated.insert(unified_ref, (block.clone(), rows));
                }
            }
        }
        Ok(updated)
    }

    /// Run `query` against the CURRENT (overlay-applied) indexed text of the
    /// given updated rows, using a throwaway in-memory text index with the
    /// same columns and tokenizer as the real one. Returned RowIds use the
    /// unified block refs, like the main hits.
    ///
    /// Scores come from this small corpus's BM25 statistics, so they are not
    /// strictly comparable with main-index scores; which rows MATCH is exact.
    async fn search_updated_rows(
        &self,
        updated: &UpdatedIndexRows,
        query: &str,
        limit: usize,
    ) -> datafusion::common::Result<Vec<(RowId, f64)>> {
        let tokenizer = self
            .index_def
            .index_type()
            .tokenizer()
            .cloned()
            .unwrap_or_default();
        let index_cols = self.index_def.column_ids().to_vec();
        let field_names: Vec<String> = index_cols
            .iter()
            .map(bundle_schema::generate_internal_name)
            .collect();
        let mut builder = bundlebase_index::TextIndexBuilder::new(
            self.index_def.name(),
            &field_names,
            &tokenizer,
        )
        .map_err(DataFusionError::External)?;
        let mut docs = 0usize;

        for (&unified_ref, (block, rows)) in updated {
            // Block column positions of the indexed columns (None if absent).
            let positions: Vec<Option<usize>> = index_cols
                .iter()
                .map(|cid| block.column_ids().iter().position(|c| c == cid))
                .collect();
            let projection: Vec<usize> = positions.iter().flatten().copied().collect();
            if projection.is_empty() {
                continue;
            }
            let projected_ids: Vec<crate::object_id::ColumnId> =
                projection.iter().map(|&p| block.column_ids()[p]).collect();
            let overlays = block.update_overlays();
            let mut stream = block
                .reader()
                .extract_rowids_stream(
                    ObjectIdAlias::from(unified_ref),
                    self.ctx.clone(),
                    Some(&projection),
                )
                .await
                .map_err(DataFusionError::External)?;
            while let Some(batch_result) = stream.next().await {
                let rowid_batch = batch_result.map_err(DataFusionError::External)?;
                let row_numbers: Vec<u32> =
                    rowid_batch.row_ids.iter().map(|r| r.row_number()).collect();
                if !row_numbers.iter().any(|rn| rows.contains(rn)) {
                    continue;
                }
                let batch = crate::bundle::update_overlay::UpdateOverlay::apply_to_batch(
                    &overlays,
                    &rowid_batch.batch,
                    &projected_ids,
                    &row_numbers,
                )
                .map_err(DataFusionError::External)?;
                for (i, row_id) in rowid_batch.row_ids.iter().enumerate() {
                    if !rows.contains(&row_id.row_number()) {
                        continue;
                    }
                    let mut values: Vec<Option<String>> = Vec::with_capacity(positions.len());
                    let mut proj_idx = 0usize;
                    for pos in &positions {
                        if pos.is_none() {
                            values.push(None);
                            continue;
                        }
                        let scalar = datafusion::common::ScalarValue::try_from_array(
                            batch.column(proj_idx),
                            i,
                        )?;
                        proj_idx += 1;
                        values.push(match scalar {
                            datafusion::common::ScalarValue::Utf8(Some(s))
                            | datafusion::common::ScalarValue::LargeUtf8(Some(s))
                            | datafusion::common::ScalarValue::Utf8View(Some(s)) => Some(s),
                            _ => None,
                        });
                    }
                    if values.iter().any(Option::is_some) {
                        builder
                            .add_document(&values, *row_id)
                            .map_err(DataFusionError::External)?;
                        docs += 1;
                    }
                }
            }
        }

        if docs == 0 {
            return Ok(Vec::new());
        }
        let index = builder.finish().map_err(DataFusionError::External)?;
        let hits = index
            .search(query, limit)
            .map_err(DataFusionError::External)?;
        Ok(hits
            .into_iter()
            .map(|hit| (hit.row_id, hit.score as f64))
            .collect())
    }

    fn empty_exec(
        &self,
        schema: &SchemaRef,
        projection: Option<&Vec<usize>>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        let projected_schema = project_schema(schema, projection)?;
        let data_source = EmptySearchDataSource { projected_schema };
        Ok(Arc::new(DataSourceExec::new(Arc::new(data_source))))
    }
}

/// DataSource that streams search results by scanning blocks on demand.
///
/// The Tantivy search phase (producing `score_map`) completes in `scan()`,
/// but the data-fetching phase is deferred to `open()` so rows are streamed
/// rather than materialized into a `Vec<RecordBatch>`.
#[derive(Clone)]
struct SearchDataSource {
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
    bundle_schema: bundle_schema::BundleSchema,
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

impl SearchDataSource {
    fn raw_stream(&self) -> SendableRecordBatchStream {
        let schema = self.physical_output_schema.clone();
        let physical_output_schema = self.physical_output_schema.clone();
        let score_map = self.score_map.clone();
        let block_id_to_ref = self.block_id_to_ref.clone();
        let ctx = self.ctx.clone();
        let bundle_schema = self.bundle_schema.clone();

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

        // Build a mapping from ColumnId → position in unified schema.
        // With shared ColumnIds across blocks, we can align by ID directly.
        let physical_col_ids = bundle_schema.physical_column_ids();
        let num_physical_cols = physical_output_schema.fields().len() - 1;
        let unified_id_to_pos: HashMap<crate::object_id::ColumnId, usize> = physical_col_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();

        let streams =
            stream::iter(blocks_to_scan)
                .map(move |(block, block_ref_idx)| {
                    let ctx = ctx.clone();
                    let score_map = score_map.clone();
                    let physical_output_schema = physical_output_schema.clone();
                    let unified_id_to_pos = unified_id_to_pos.clone();
                    stream::once(async move {
                        let reader = block.reader();
                        let block_ref = ObjectIdAlias::from(block_ref_idx);
                        // Block-level edit state: the raw reader below knows nothing of
                        // tombstones or update overlays, and search output does not pass
                        // through the scan-level appliers (bb-fqm.2).
                        let deleted = block.deleted_rows();
                        let overlays = block.update_overlays();

                        // Build mapping: unified position → block column index (None if block lacks this column)
                        let mut unified_to_block: Vec<Option<usize>> =
                            vec![None; num_physical_cols];
                        for (block_idx, col_id) in block.column_ids().iter().enumerate() {
                            if let Some(&unified_pos) = unified_id_to_pos.get(col_id) {
                                unified_to_block[unified_pos] = Some(block_idx);
                            }
                        }

                        let rowid_stream = reader
                            .extract_rowids_stream(block_ref, ctx.clone(), None)
                            .await
                            .map_err(DataFusionError::External)?;
                        let column_ids = block.column_ids().to_vec();
                        Ok::<_, DataFusionError>(rowid_stream.map(move |batch_result| {
                    let rowid_batch = batch_result.map_err(DataFusionError::External)?;
                    let row_ids = &rowid_batch.row_ids;
                    // Apply update overlays to the batch VALUES (positions are
                    // untouched, so row_ids indexing below stays valid).
                    let batch = if overlays.is_empty() {
                        rowid_batch.batch.clone()
                    } else {
                        let row_numbers: Vec<u32> =
                            row_ids.iter().map(|r| r.row_number()).collect();
                        crate::bundle::update_overlay::UpdateOverlay::apply_to_batch(
                            &overlays,
                            &rowid_batch.batch,
                            &column_ids,
                            &row_numbers,
                        )
                        .map_err(DataFusionError::External)?
                    };
                    let batch = &batch;

                    // Find matching rows in this batch
                    let mut matching_indices: Vec<usize> = Vec::new();
                    let mut matching_scores: Vec<f64> = Vec::new();

                    for (idx, row_id) in row_ids.iter().enumerate() {
                        // Tombstoned rows must not resurrect via the search index.
                        if deleted.binary_search(&row_id.row_number()).is_ok() {
                            continue;
                        }
                        if let Some(&score) = score_map.get(&row_id.as_u64()) {
                            matching_indices.push(idx);
                            matching_scores.push(score);
                        }
                    }

                    if matching_indices.is_empty() {
                        return Ok(None);
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
                                let filtered =
                                    compute::take(&batch.columns()[*block_idx], &indices, None)
                                        .map_err(|e| {
                                            DataFusionError::ArrowError(Box::new(e), None)
                                        })?;
                                aligned_columns.push(filtered);
                            }
                            None => {
                                // Block doesn't have this column — insert null array of correct type
                                let field = physical_output_schema.field(unified_pos);
                                let null_array =
                                    arrow::array::new_null_array(field.data_type(), num_matching);
                                aligned_columns.push(null_array);
                            }
                        }
                    }

                    // Append score column
                    let score_array = Float64Array::from(matching_scores);
                    aligned_columns.push(Arc::new(score_array));

                    let result_batch =
                        RecordBatch::try_new(physical_output_schema.clone(), aligned_columns)
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;

                    Ok(Some(result_batch))
                }).filter_map(|result| async move { result.transpose() }))
                    })
                    .flat_map(|result| match result {
                        Ok(stream) => futures::future::Either::Left(stream),
                        Err(error) => {
                            futures::future::Either::Right(stream::once(async move { Err(error) }))
                        }
                    })
                })
                .flatten();
        Box::pin(RecordBatchStreamAdapter::new(schema, streams))
    }
}

#[derive(Debug)]
struct SearchRawPartition(SearchDataSource);

impl PartitionStream for SearchRawPartition {
    fn schema(&self) -> &SchemaRef {
        &self.0.physical_output_schema
    }
    fn execute(&self, _ctx: Arc<TaskContext>) -> SendableRecordBatchStream {
        self.0.raw_stream()
    }
}

impl DataSource for SearchDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let physical_output_schema = self.physical_output_schema.clone();
        let projected_schema = self.projected_schema.clone();
        let ctx = self.ctx.clone();
        let projection = self.projection.clone();
        let operations = self.operations.clone();

        let source = self.clone();
        let result_stream = stream::once(async move {
            let mut config = SessionConfig::new();
            config.options_mut().sql_parser.enable_ident_normalization = false;
            let op_ctx = SessionContext::new_with_config_rt(config, ctx.runtime_env());
            // Replay operations with the same functions used to plan their schema.
            let state = ctx.state();
            for udf in state.scalar_functions().values() {
                op_ctx.register_udf(udf.as_ref().clone());
            }
            for udaf in state.aggregate_functions().values() {
                op_ctx.register_udaf(udaf.as_ref().clone());
            }
            for udwf in state.window_functions().values() {
                op_ctx.register_udwf(udwf.as_ref().clone());
            }
            let table = StreamingTable::try_new(
                physical_output_schema.clone(),
                vec![Arc::new(SearchRawPartition(source))],
            )?;
            op_ctx.register_table("bundle", Arc::new(table))?;
            let mut df = op_ctx.table("bundle").await?;
            let mut bundle_schema = bundle_schema::BundleSchema::initial(&operations);
            let cast_active = resolve_cast_ops(&operations);
            for (op, active) in operations.iter().zip(cast_active.iter()) {
                if *active {
                    df = op
                        .apply_dataframe(df, Arc::new(op_ctx.clone()), &mut bundle_schema)
                        .await
                        .map_err(DataFusionError::External)?;
                }
            }
            df = bundle_schema
                .rename_to_real_names(df)
                .map_err(DataFusionError::External)?;
            let final_stream = df.execute_stream().await?;
            Ok::<_, DataFusionError>(final_stream.map(move |batch| {
                let batch = batch?;
                match &projection {
                    Some(projection) => batch.project(projection).map_err(Into::into),
                    None => Ok(batch),
                }
            }))
        })
        .flat_map(|result| match result {
            Ok(stream) => futures::future::Either::Left(stream),
            Err(error) => futures::future::Either::Right(stream::once(async move { Err(error) })),
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            projected_schema,
            result_stream,
        )))
    }

    fn fmt_as(
        &self,
        _t: datafusion::physical_plan::DisplayFormatType,
        f: &mut fmt::Formatter,
    ) -> fmt::Result {
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
    ) -> datafusion::common::Result<Arc<Statistics>> {
        Ok(Arc::new(Statistics::new_unknown(&self.projected_schema)))
    }

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> datafusion::common::Result<TreeNodeRecursion>,
    ) -> datafusion::common::Result<TreeNodeRecursion> {
        // Search results are fixed at scan time; no filters are pushed into this source.
        Ok(TreeNodeRecursion::Continue)
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

    fn fmt_as(
        &self,
        _t: datafusion::physical_plan::DisplayFormatType,
        f: &mut fmt::Formatter,
    ) -> fmt::Result {
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
    ) -> datafusion::common::Result<Arc<Statistics>> {
        Ok(Arc::new(Statistics::new_unknown(&self.projected_schema)))
    }

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> datafusion::common::Result<TreeNodeRecursion>,
    ) -> datafusion::common::Result<TreeNodeRecursion> {
        // Search results are fixed at scan time; no filters are pushed into this source.
        Ok(TreeNodeRecursion::Continue)
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
