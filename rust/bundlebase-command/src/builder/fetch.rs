//! Fetch command implementations.

use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::{
    AnyOperation, AttachBlockOp, BatchedSource, DetachBlockOp, SourceInfo,
};
use bundlebase::bundle::BundleFacade;
use bundlebase::source::{FetchAction, FetchResults, SyncMode};
use bundlebase::ExpectedColumn;
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_common::progress::ProgressScope;
use bundlebase_common::BundlebaseError;
use bundlebase_data::attach_format::AttachFormat;
use bundlebase_data::BlockId;
use bundlebase_data::ObjectId;
use futures::StreamExt;
use log::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;

/// Command to fetch from sources for a specific pack.
#[derive(Debug, Clone)]
pub struct FetchCommand {
    /// The pack to fetch sources for (e.g. "base", or a join name)
    pub pack: String,
    /// Sync mode for the fetch operation
    pub mode: SyncMode,
    /// If true, only compute what would change without actually executing
    pub dry_run: bool,
}

impl FetchCommand {
    /// Create a new FetchCommand.
    pub fn new(pack: String, mode: SyncMode) -> Self {
        Self {
            pack,
            mode,
            dry_run: false,
        }
    }

    /// Create a new FetchCommand with dry_run flag.
    pub fn new_with_dry_run(pack: String, mode: SyncMode, dry_run: bool) -> Self {
        Self {
            pack,
            mode,
            dry_run,
        }
    }
}

impl CommandParsing for FetchCommand {
    fn rule() -> Rule {
        Rule::fetch_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut pack = None;
        let mut mode = None;
        let mut dry_run = false;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::identifier => {
                    pack = Some(extract_identifier(&inner_pair));
                }
                Rule::fetch_mode => {
                    mode = Some(SyncMode::from_arg(inner_pair.as_str())?);
                }
                Rule::fetch_dry_run => {
                    dry_run = true;
                }
                _ => {}
            }
        }

        let pack =
            pack.ok_or_else(|| BundlebaseError::from("FETCH statement missing pack name"))?;
        let mode = mode.ok_or_else(|| BundlebaseError::from("FETCH statement missing mode"))?;

        Ok(FetchCommand::new_with_dry_run(pack, mode, dry_run))
    }

    fn to_statement(&self) -> String {
        if self.dry_run {
            format!(
                "FETCH {} {} DRY RUN",
                quote_identifier(&self.pack),
                self.mode
            )
        } else {
            format!("FETCH {} {}", quote_identifier(&self.pack), self.mode)
        }
    }
}

impl BundleBuilderCommand for FetchCommand {
    type Output = Vec<FetchResults>;

    async fn execute(
        self: Box<Self>,
        builder: &BundleBuilder,
    ) -> Result<Vec<FetchResults>, BundlebaseError> {
        let pack_name = self.pack.clone();
        let pack_id = builder.resolve_pack_id(Some(&self.pack))?;

        let mode = self.mode;

        let sources = builder.bundle().get_sources_for_pack(&pack_id);
        if sources.is_empty() {
            return Err(format!("No sources defined for pack '{}'", pack_name).into());
        }

        let mut results = Vec::new();
        for source in sources {
            let result = fetch_from_source(builder, &source, &pack_id, &pack_name, mode).await?;
            results.push(result);
        }

        Ok(results)
    }
}

/// Command to fetch from all defined sources.
#[derive(Debug, Clone)]
pub struct FetchAllCommand {
    /// Sync mode for the fetch operation
    pub mode: SyncMode,
    /// If true, only compute what would change without actually executing
    pub dry_run: bool,
}

impl FetchAllCommand {
    /// Create a new FetchAllCommand.
    pub fn new(mode: SyncMode) -> Self {
        Self {
            mode,
            dry_run: false,
        }
    }

    /// Create a new FetchAllCommand with dry_run flag.
    pub fn new_with_dry_run(mode: SyncMode, dry_run: bool) -> Self {
        Self { mode, dry_run }
    }
}

impl CommandParsing for FetchAllCommand {
    fn rule() -> Rule {
        Rule::fetch_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut mode = None;
        let mut dry_run = false;
        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::fetch_mode => {
                    mode = Some(SyncMode::from_arg(inner_pair.as_str())?);
                }
                Rule::fetch_dry_run => {
                    dry_run = true;
                }
                _ => {}
            }
        }

        let mode = mode.ok_or_else(|| BundlebaseError::from("FETCH ALL statement missing mode"))?;

        Ok(FetchAllCommand::new_with_dry_run(mode, dry_run))
    }

    fn to_statement(&self) -> String {
        if self.dry_run {
            format!("FETCH ALL {} DRY RUN", self.mode)
        } else {
            format!("FETCH ALL {}", self.mode)
        }
    }
}

impl BundleBuilderCommand for FetchAllCommand {
    type Output = Vec<FetchResults>;

    async fn execute(
        self: Box<Self>,
        builder: &BundleBuilder,
    ) -> Result<Vec<FetchResults>, BundlebaseError> {
        let mode = self.mode;

        // Collect sources with their pack info to avoid borrow issues
        let sources_with_packs: Vec<_> = builder
            .bundle()
            .sources()
            .values()
            .map(|source| {
                let pack_name = builder
                    .bundle()
                    .pack_name(source.pack())
                    .unwrap_or("base".to_string());
                let pack_id = *source.pack();
                (source.clone(), pack_id, pack_name)
            })
            .collect();

        let mut results = Vec::new();
        for (source, pack_id, pack_name) in sources_with_packs {
            let result = fetch_from_source(builder, &source, &pack_id, &pack_name, mode).await?;
            results.push(result);
        }

        Ok(results)
    }
}

/// Helper to fetch from a single source.
async fn fetch_from_source(
    builder: &BundleBuilder,
    source: &Arc<bundlebase::bundle::Source>,
    pack_id: &ObjectId,
    pack_name: &str,
    mode: SyncMode,
) -> Result<FetchResults, BundlebaseError> {
    let source_id = *source.id();

    // Look up expected_schema and json_* read options from the CreateSourceOp in operations.
    let ops = builder.operations();
    let (expected_schema, json_read_options) = ops
        .iter()
        .find_map(|op| match op {
            AnyOperation::CreateSource(src) if src.id == source_id => Some((
                src.expected_schema.clone(),
                super::extract_json_opts(&src.args),
            )),
            _ => None,
        })
        .unwrap_or((None, None));
    let connector = source.connector().to_string();
    let source_url = source.args().get("url").cloned().unwrap_or_default();

    // Skip rows_before: full scan is too expensive for large bundles
    let rows_before: u64 = 0;

    let actions = source.fetch(builder, mode).await?;

    // Separate Add actions (parallelizable setup) from Replace/Remove (sequential)
    let mut add_actions = Vec::new();
    let mut sequential_actions = Vec::new();
    for action in actions.iter() {
        match action {
            FetchAction::Add(data) => add_actions.push(data.clone()),
            _ => sequential_actions.push(action.clone()),
        }
    }

    let total = actions.len();
    let progress = ProgressScope::new(
        &format!("Applying {} fetch actions for '{}'", total, pack_name),
        Some(total as u64),
    );

    // Phase 1: Prepare Add operations concurrently (I/O-heavy setup: hash, schema, stats)
    let setup_progress = ProgressScope::new(
        &format!("Preparing {} new attachments", add_actions.len()),
        Some(add_actions.len() as u64),
    );

    // Share a single SharedAttachContext across all parallel attaches in
    // this batch. Without this, every parallel setup() generates fresh
    // ColumnIds for the same logical columns AND re-writes the same schema
    // file once per attach, blowing up both the merged schema and disk I/O.
    let shared_ctx = builder.shared_attach_context();

    let prepared_adds: Vec<Result<(AttachBlockOp, String), BundlebaseError>> =
        futures::stream::iter(add_actions.into_iter().enumerate())
            .map(|(idx, data)| {
                let json_read_options = &json_read_options;
                let expected_schema = &expected_schema;
                let setup_progress = &setup_progress;
                let shared_ctx = shared_ctx.clone();
                async move {
                    let (final_location, format, hash) = resolve_attach_location(
                        builder,
                        &data.attach_location,
                        data.hash.clone(),
                        json_read_options.as_ref(),
                    )
                    .await?;
                    let op = AttachBlockOp::setup(
                        pack_id,
                        &final_location,
                        format,
                        hash.as_deref(),
                        Some(SourceInfo {
                            id: source_id,
                            batch_sources: vec![BatchedSource {
                                location: data.source_location.clone(),
                                version: data.version.clone(),
                            }],
                        }),
                        expected_schema.as_deref(),
                        builder,
                        Some(&shared_ctx),
                    )
                    .await?;
                    validate_schema_against_expected(
                        &op,
                        expected_schema.as_deref(),
                        &data.attach_location,
                    );
                    setup_progress.update((idx + 1) as u64, Some(&data.attach_location));
                    Ok((op, data.attach_location.clone()))
                }
            })
            .buffer_unordered(100)
            .collect()
            .await;

    // Collect prepared ops, propagating errors
    let mut prepared_ops: Vec<(AttachBlockOp, String)> = Vec::with_capacity(prepared_adds.len());
    for result in prepared_adds {
        prepared_ops.push(result?);
    }

    // Phase 2: Batch small files if min_batch_bytes is configured on the source.
    let final_ops = if let Some(min_batch_bytes) = source.min_batch_bytes() {
        batch_small_ops(prepared_ops, min_batch_bytes, source_id, builder).await?
    } else {
        prepared_ops
    };

    // Phase 3: Apply prepared Add operations sequentially (fast, no I/O)
    let mut applied = 0;
    for (op, attach_location) in final_ops {
        builder.apply_operation(op.into()).await?;
        applied += 1;
        progress.update(applied as u64, None);
        info!("Fetched {} to {}", attach_location, pack_name);
    }

    // Phase 4: Process Replace/Remove actions sequentially (depend on bundle state)
    for action in &sequential_actions {
        match action {
            FetchAction::Replace {
                old_source_location,
                data,
            } => {
                let bundle_snapshot = builder.bundle().clone();
                let old_location = find_block_location_by_source(
                    &bundle_snapshot,
                    &source_id,
                    old_source_location,
                )?;
                let detach_op = DetachBlockOp::setup(&old_location, builder).await?;
                builder.apply_operation(detach_op.into()).await?;

                let (final_location, format, hash) = resolve_attach_location(
                    builder,
                    &data.attach_location,
                    data.hash.clone(),
                    json_read_options.as_ref(),
                )
                .await?;
                let op = AttachBlockOp::setup(
                    pack_id,
                    &final_location,
                    format,
                    hash.as_deref(),
                    Some(SourceInfo {
                        id: source_id,
                        batch_sources: vec![BatchedSource {
                            location: data.source_location.clone(),
                            version: data.version.clone(),
                        }],
                    }),
                    expected_schema.as_deref(),
                    builder,
                    None,
                )
                .await?;
                validate_schema_against_expected(
                    &op,
                    expected_schema.as_deref(),
                    &data.attach_location,
                );
                builder.apply_operation(op.into()).await?;
                info!("Replaced {} in {}", data.attach_location, pack_name);
            }
            FetchAction::Remove { source_location } => {
                let bundle_snapshot = builder.bundle().clone();
                let location =
                    find_block_location_by_source(&bundle_snapshot, &source_id, source_location)?;
                let detach_op = DetachBlockOp::setup(&location, builder).await?;
                builder.apply_operation(detach_op.into()).await?;
                info!("Removed {} from {}", location, pack_name);
            }
            _ => {}
        }
        applied += 1;
        progress.update(applied as u64, None);
    }

    let processed_actions = actions;

    // Skip rows_after: full scan is too expensive for large bundles
    let rows_after: u64 = 0;

    let mut results = FetchResults::from_actions(
        connector,
        source_url,
        pack_name.to_string(),
        processed_actions,
    );
    results.rows_before = rows_before;
    results.rows_after = rows_after;
    Ok(results)
}

/// Resolve the final attach location and format for a fetched file.
///
/// If `json_opts` is present, converts the JSON file to Parquet in the data dir and
/// returns the Parquet path. Otherwise detects the format from the file directly.
/// Returns `(location, format, hash)`.
async fn resolve_attach_location(
    builder: &BundleBuilder,
    location: &str,
    original_hash: Option<String>,
    json_opts: Option<&HashMap<String, String>>,
) -> Result<(String, AttachFormat, Option<String>), BundlebaseError> {
    if let Some(opts) = json_opts {
        let (parquet_location, parquet_hash) = builder
            .convert_json_attachment_to_parquet(location, opts)
            .await?;
        Ok((parquet_location, AttachFormat::Parquet, Some(parquet_hash)))
    } else {
        let temp_reader = builder
            .bundle()
            .reader_factory
            .detect(location, &BlockId::generate(), builder)
            .await?;
        Ok((location.to_string(), temp_reader.format(), original_hash))
    }
}

/// Find the current location of a block that was attached from a source.
fn find_block_location_by_source(
    bundle: &Bundle,
    source_id: &ObjectId,
    source_location: &str,
) -> Result<String, BundlebaseError> {
    use bundlebase::bundle::operation::AnyOperation;

    // First, check ReplaceBlockOp operations (in reverse order to get most recent)
    let operations = bundle.operations.read();
    for op in operations.iter().rev() {
        if let AnyOperation::ReplaceBlock(replace) = op {
            if let Some(ref info) = replace.source_info {
                if &info.id == source_id
                    && info
                        .batch_sources
                        .iter()
                        .any(|source| source.location == source_location)
                {
                    return Ok(replace.new_location.clone());
                }
            }
        }
    }

    // If not found in ReplaceBlockOp, check AttachBlockOp
    operations
        .iter()
        .find_map(|op| {
            if let AnyOperation::AttachBlock(attach) = op {
                if let Some(ref info) = attach.source_info {
                    if &info.id == source_id
                        && info
                            .batch_sources
                            .iter()
                            .any(|b| b.location == source_location)
                    {
                        return Some(attach.location.clone());
                    }
                }
            }
            None
        })
        .ok_or_else(|| format!("No block found for source_location '{}'", source_location).into())
}

/// Public wrapper for `batch_small_ops` — used by create_source.rs.
pub(crate) async fn batch_small_ops_public(
    ops: Vec<(AttachBlockOp, String)>,
    batch_bytes: usize,
    source_id: ObjectId,
    builder: &BundleBuilder,
) -> Result<Vec<(AttachBlockOp, String)>, BundlebaseError> {
    batch_small_ops(ops, batch_bytes, source_id, builder).await
}

/// Group ops into chunks where cumulative bytes reach `batch_bytes` threshold.
/// Files larger than threshold become single-item chunks (passed through unchanged).
fn group_by_size(
    ops: Vec<(AttachBlockOp, String)>,
    batch_bytes: usize,
) -> Vec<Vec<(AttachBlockOp, String)>> {
    let mut chunks: Vec<Vec<(AttachBlockOp, String)>> = Vec::new();
    let mut current: Vec<(AttachBlockOp, String)> = Vec::new();
    let mut current_bytes: usize = 0;

    for (op, loc) in ops {
        let op_bytes = op.bytes.unwrap_or(0);
        // If this file alone exceeds the threshold, emit it as its own chunk
        if op_bytes >= batch_bytes {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
            chunks.push(vec![(op, loc)]);
            continue;
        }
        // If adding this file would exceed the threshold and current is non-empty, flush first
        if !current.is_empty() && current_bytes + op_bytes > batch_bytes {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.push((op, loc));
        current_bytes += op_bytes;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Batch small files into larger combined blocks.
///
/// Groups prepared AttachBlockOps into chunks where the cumulative raw bytes
/// reach `batch_bytes` threshold. Files larger than the threshold are passed
/// through as single-file chunks. Only parquet files are batched — other formats
/// (JSONL, CSV, etc.) are passed through unchanged.
async fn batch_small_ops(
    ops: Vec<(AttachBlockOp, String)>,
    batch_bytes: usize,
    _source_id: ObjectId,
    builder: &BundleBuilder,
) -> Result<Vec<(AttachBlockOp, String)>, BundlebaseError> {
    use bundlebase::source::{read_parquet_batches, write_merged_parquet};

    if batch_bytes == 0 || ops.len() <= 1 {
        return Ok(ops);
    }

    // Only parquet files are batched. Other formats pass through unchanged.
    let mut parquet_ops = Vec::new();
    let mut other_ops = Vec::new();
    for (op, loc) in ops {
        match op.format {
            AttachFormat::Parquet => parquet_ops.push((op, loc)),
            _ => other_ops.push((op, loc)),
        }
    }

    let total_batchable = parquet_ops.len();
    let progress = ProgressScope::new(
        &format!(
            "Batching {} parquet files (threshold {} bytes)",
            total_batchable, batch_bytes
        ),
        Some(total_batchable as u64),
    );

    let mut result = other_ops;
    let mut processed = 0usize;

    // Batch parquet files by size
    let parquet_chunks = group_by_size(parquet_ops, batch_bytes);
    for (batch_idx, chunk) in parquet_chunks.into_iter().enumerate() {
        if chunk.len() == 1 {
            result.push(chunk.into_iter().next().unwrap());
            processed += 1;
            progress.update(processed as u64, None);
            continue;
        }

        let data_dir = builder.bundle().data_dir();
        let first_op = &chunk[0].0;
        let batch_sources = build_batch_sources(&chunk);

        // First pass: read all parquet files in parallel (I/O bound)
        let data_dir_clone = builder.bundle().data_dir();
        let locations: Vec<String> = chunk.iter().map(|(op, _)| op.location.clone()).collect();
        let read_results: Vec<
            Result<
                (
                    arrow_schema::SchemaRef,
                    Vec<arrow::record_batch::RecordBatch>,
                ),
                BundlebaseError,
            >,
        > = futures::stream::iter(locations.into_iter())
            .map(|location| {
                let dir = data_dir_clone.clone();
                async move { read_parquet_batches(&location, dir.as_ref()).await }
            })
            .buffer_unordered(50)
            .collect()
            .await;

        let mut per_file_batches: Vec<Vec<arrow::record_batch::RecordBatch>> = Vec::new();
        let mut union_fields: Vec<arrow_schema::Field> = Vec::new();
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut total_rows: usize = 0;
        let mut total_bytes: usize = 0;

        for (read_result, (op, _)) in read_results.into_iter().zip(chunk.iter()) {
            let (schema, batches) = read_result?;
            for field in schema.fields() {
                if seen_names.insert(field.name().clone()) {
                    union_fields.push(field.as_ref().clone());
                }
            }
            for batch in &batches {
                total_rows += batch.num_rows();
            }
            per_file_batches.push(batches);
            total_bytes += op.bytes.unwrap_or(0);
        }

        let union_schema = Arc::new(arrow_schema::Schema::new(union_fields));

        // Second pass: align each batch to the union schema, adding null columns for missing fields
        let mut all_batches = Vec::new();
        for batches in per_file_batches {
            for batch in batches {
                let aligned = align_batch_to_schema(&batch, &union_schema)?;
                all_batches.push(aligned);
            }
        }

        let schema = union_schema;
        let write_result = write_merged_parquet(all_batches, data_dir.as_ref()).await?;
        let merged_location = data_dir.relative_path(write_result.file.as_ref())?;
        let merged_hash = write_result.hash;
        // Read back the object-store version so DataBlock::validate_version()
        // uses the same scheme (e_tag / sequential counter) that it will see
        // at query time — not the content hash used to name the file.
        let merged_version = write_result.file.version().await?;

        // Build a column_ids list that matches the *union* schema (one ID per
        // field in the merged parquet). The per-chunk ops may have shorter
        // column_ids lists (only their own schemas); we need to extend with
        // fresh IDs for fields that only appear in later chunk members.
        // Re-use the shared attach context so names seen across this and
        // any other batches resolve to the same ColumnId.
        let merged_shared_ctx = builder.shared_attach_context();
        let merged_column_ids: Vec<bundlebase_common::ColumnId> = {
            // Seed the name→id map from the source ops in this chunk so
            // fields they covered keep their original IDs.
            {
                let mut name_to_id = merged_shared_ctx.name_to_id.lock();
                for (src_op, _) in chunk.iter() {
                    if let Some(src_schema) = src_op.schema_cache.as_ref() {
                        for (field, id) in src_schema
                            .fields()
                            .iter()
                            .zip(src_op.column_ids_cache.iter())
                        {
                            name_to_id.entry(field.name().clone()).or_insert(*id);
                        }
                    }
                }
            }
            let mut name_to_id = merged_shared_ctx.name_to_id.lock();
            schema
                .fields()
                .iter()
                .map(|f| {
                    *name_to_id
                        .entry(f.name().clone())
                        .or_insert_with(bundlebase_common::ColumnId::generate)
                })
                .collect()
        };

        // Persist the merged schema and (union-schema-sized) column-id list
        // as sidecar files using the same dedup mechanism as per-block setup.
        let schema_cache = schema;
        let schema = bundlebase::bundle::operation::AttachBlockOp::write_schema_file(
            &schema_cache,
            &merged_shared_ctx,
            data_dir.as_ref(),
        )
        .await?;
        let column_ids = bundlebase::bundle::operation::AttachBlockOp::write_column_ids_file(
            &merged_column_ids,
            &merged_shared_ctx,
            data_dir.as_ref(),
        )
        .await?;

        let chunk_len = chunk.len();
        let merged_op = build_merged_op(
            first_op,
            &merged_location,
            &merged_version,
            &merged_hash,
            AttachFormat::Parquet,
            &batch_sources,
            Some(total_rows),
            Some(total_bytes),
            schema_cache,
            schema,
            column_ids,
            merged_column_ids,
        );
        result.push((
            merged_op,
            format!("parquet-batch-{} ({} files)", batch_idx, chunk_len),
        ));
        processed += chunk_len;
        progress.update(processed as u64, None);
        info!(
            "Batched {} parquet files into {}",
            chunk_len, merged_location
        );
    }

    Ok(result)
}

/// Align a RecordBatch to a target schema by adding null columns for missing fields.
fn align_batch_to_schema(
    batch: &arrow::record_batch::RecordBatch,
    target: &arrow_schema::SchemaRef,
) -> Result<arrow::record_batch::RecordBatch, BundlebaseError> {
    let batch_schema = batch.schema();
    let num_rows = batch.num_rows();
    let mut columns: Vec<Arc<dyn arrow::array::Array>> = Vec::with_capacity(target.fields().len());
    for field in target.fields() {
        if let Some((i, _)) = batch_schema
            .fields()
            .iter()
            .enumerate()
            .find(|(_, f)| f.name() == field.name())
        {
            columns.push(batch.column(i).clone());
        } else {
            // Add null column matching target type
            let null_array = arrow::array::new_null_array(field.data_type(), num_rows);
            columns.push(null_array);
        }
    }
    arrow::record_batch::RecordBatch::try_new(target.clone(), columns)
        .map_err(|e| BundlebaseError::from(format!("Failed to align batch: {}", e)))
}

/// Build BatchedSource entries from a chunk of ops.
fn build_batch_sources(chunk: &[(AttachBlockOp, String)]) -> Vec<BatchedSource> {
    chunk
        .iter()
        .flat_map(|(op, _)| {
            op.source_info
                .as_ref()
                .map(|si| si.batch_sources.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// Build a merged AttachBlockOp from a batch.
#[allow(clippy::too_many_arguments)]
fn build_merged_op(
    first_op: &AttachBlockOp,
    merged_location: &str,
    merged_version: &str,
    merged_hash: &str,
    format: AttachFormat,
    batch_sources: &[BatchedSource],
    num_rows: Option<usize>,
    bytes: Option<usize>,
    schema_cache: arrow_schema::SchemaRef,
    schema: String,
    column_ids: String,
    column_ids_cache: Vec<bundlebase_common::ColumnId>,
) -> AttachBlockOp {
    let merged_source_info = first_op.source_info.as_ref().map(|si| SourceInfo {
        id: si.id,
        batch_sources: if batch_sources.is_empty() {
            si.batch_sources.clone()
        } else {
            batch_sources.to_vec()
        },
    });

    AttachBlockOp {
        id: BlockId::generate(),
        pack: first_op.pack,
        location: merged_location.to_string(),
        format,
        read_options: None,
        version: merged_version.to_string(),
        hash: merged_hash.to_string(),
        source_info: merged_source_info,
        layout: None,
        num_rows,
        bytes,
        schema,
        column_ids,
        schema_cache: Some(schema_cache),
        column_ids_cache,
    }
}

/// Validate a fetched block's schema against the source's expected schema.
///
/// Emits warnings for missing/type-changed columns, and info for new columns.
/// Non-blocking — fetch proceeds regardless.
fn validate_schema_against_expected(
    op: &AttachBlockOp,
    expected_schema: Option<&[ExpectedColumn]>,
    location: &str,
) {
    let expected = match expected_schema {
        Some(e) if !e.is_empty() => e,
        _ => return,
    };
    let fetched_schema = match &op.schema_cache {
        Some(s) => s,
        None => return,
    };

    // Build map of fetched column name → data type
    let fetched: std::collections::HashMap<&str, &arrow::datatypes::DataType> = fetched_schema
        .fields()
        .iter()
        .map(|f| (f.name().as_str(), f.data_type()))
        .collect();

    for col in expected {
        match fetched.get(col.name.as_str()) {
            None => {
                warn!(
                    "Expected column '{}' ({:?}) not found in fetched data at '{}'",
                    col.name, col.data_type, location
                );
            }
            Some(&fetched_type) if fetched_type != &col.data_type => {
                warn!(
                    "Column '{}' type changed from {:?} to {:?} in fetched data at '{}'",
                    col.name, col.data_type, fetched_type, location
                );
            }
            _ => {}
        }
    }

    // New columns (in fetched but not in expected)
    let expected_names: std::collections::HashSet<&str> =
        expected.iter().map(|c| c.name.as_str()).collect();
    for field in fetched_schema.fields() {
        if !expected_names.contains(field.name().as_str()) {
            info!(
                "New column '{}' ({:?}) found in fetched data at '{}'",
                field.name(),
                field.data_type(),
                location
            );
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_fetch_base() {
        let input = "FETCH base ADD";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_pack() {
        let input = "FETCH users ADD";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "users");
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_with_mode() {
        let input = "FETCH base UPDATE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Update);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_sync_mode() {
        let input = "FETCH base SYNC";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_pack_with_mode() {
        let input = "FETCH users SYNC";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "users");
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all() {
        let input = "FETCH ALL ADD";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all_with_mode() {
        let input = "FETCH ALL UPDATE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Update);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all_sync() {
        let input = "FETCH ALL SYNC";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_base() {
        let cmd = FetchCommand::new("base".to_string(), SyncMode::Add);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH base ADD");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_with_mode() {
        let cmd = FetchCommand::new("base".to_string(), SyncMode::Update);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH base UPDATE");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Update);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_pack() {
        let cmd = FetchCommand::new("users".to_string(), SyncMode::Sync);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH users SYNC");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "users");
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_all() {
        let cmd = FetchAllCommand::new(SyncMode::Add);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH ALL ADD");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_all_with_mode() {
        let cmd = FetchAllCommand::new(SyncMode::Sync);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH ALL SYNC");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }
}
