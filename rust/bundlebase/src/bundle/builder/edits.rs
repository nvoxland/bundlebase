//! DELETE/UPDATE edit machinery for [`BundleBuilder`] (ADR-009).
//!
//! Pending edits accumulate here between commits and are mirrored into the
//! live [`DataBlock`]s for in-session visibility. Every edit is keyed by the
//! block's stable `(BlockId, version)` plus the row number within that block —
//! never by the block's position in the base pack, which shifts when a block
//! is DETACHed or REPLACEd (in-session, or by a concurrent writer whose
//! commit is replayed first).
//!
//! Undo / failed-command rollback is delta-based (bb-8lk): each change records
//! only what it added (newly tombstoned rows, the prior value of every update
//! cell it wrote, and how many WHERE clauses it appended) and is reversed by
//! un-applying exactly that, so memory is proportional to the edits made, not
//! to `#changes × pending size`.

use super::BundleBuilder;
use crate::bundle::bundle_schema;
use crate::bundle::operation::{AnyOperation, DeleteOp, UpdateDataOp};
use crate::bundle::tombstone::{self, RowSelection};
use crate::bundle::update_overlay::{self, PendingUpdates, UpdateOverlay};
use crate::bundle::{BundleFacade, DataBlock};
use crate::data::{ObjectId, ObjectIdAlias, VersionedBlockId};
use crate::object_id::ColumnId;
use crate::BundlebaseError;
use arrow::array::RecordBatch;
use arrow_schema::SchemaRef;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion::scalar::ScalarValue;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::debug;

/// What one change added to the pending accumulators — enough to reverse
/// exactly that change on undo or after a failed command (bb-8lk).
#[derive(Clone, Debug, Default)]
pub(super) struct ChangeDelta {
    /// Rows this change newly tombstoned (absent from the pending set before).
    added_deletes: RowSelection,
    /// Number of DELETE WHERE clauses this change appended.
    delete_wheres_added: usize,
    /// Prior value (`None` = cell absent) of every pending update cell this
    /// change wrote. Only the FIRST write per cell is recorded, so reverting
    /// restores the value from before the change.
    overwritten_cells: HashMap<(VersionedBlockId, u32, ColumnId), Option<ScalarValue>>,
    /// Number of UPDATE WHERE clauses this change appended.
    update_wheres_added: usize,
}

/// The builder's uncommitted DELETE/UPDATE state.
#[derive(Clone, Debug, Default)]
pub(super) struct PendingEdits {
    /// Rows tombstoned by DELETE commands, written to a tombstone on commit.
    pub(super) deletes: RowSelection,
    /// DELETE WHERE clauses, kept for the operation log only.
    pub(super) delete_wheres: Vec<String>,
    /// Updated cell values, written to an overlay parquet on commit.
    pub(super) updates: PendingUpdates,
    /// UPDATE WHERE clauses, kept for the operation log only.
    pub(super) update_wheres: Vec<String>,
    /// Delta of the change in progress.
    current: ChangeDelta,
    /// One delta per completed change in the builder's status.
    history: Vec<ChangeDelta>,
}

impl PendingEdits {
    /// Drop all pending edits and change history (after commit/reset).
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    fn add_delete(&mut self, block: &VersionedBlockId, row: u32) {
        if self.deletes.insert(block, row) {
            self.current.added_deletes.insert(block, row);
        }
    }

    fn push_delete_where(&mut self, where_clause: &str) {
        self.delete_wheres.push(where_clause.to_string());
        self.current.delete_wheres_added += 1;
    }

    fn set_cell(&mut self, block: &VersionedBlockId, row: u32, col: ColumnId, value: ScalarValue) {
        let prior = self
            .updates
            .entry(block.clone())
            .or_default()
            .entry(row)
            .or_default()
            .insert(col, value);
        self.current
            .overwritten_cells
            .entry((block.clone(), row, col))
            .or_insert(prior);
    }

    fn push_update_where(&mut self, where_clause: &str) {
        self.update_wheres.push(where_clause.to_string());
        self.current.update_wheres_added += 1;
    }

    /// Whether the change in progress has added any delete or update.
    pub(super) fn current_is_empty(&self) -> bool {
        let c = &self.current;
        c.added_deletes.is_empty()
            && c.delete_wheres_added == 0
            && c.overwritten_cells.is_empty()
            && c.update_wheres_added == 0
    }

    /// The change in progress completed: keep its delta for undo.
    pub(super) fn complete_change(&mut self) {
        let delta = std::mem::take(&mut self.current);
        self.history.push(delta);
    }

    /// The change in progress failed: reverse whatever it already added.
    pub(super) fn rollback_current(&mut self) {
        let delta = std::mem::take(&mut self.current);
        self.revert(delta);
    }

    /// Undo the most recent completed change (and anything in progress).
    pub(super) fn undo_last_change(&mut self) {
        self.rollback_current();
        if let Some(delta) = self.history.pop() {
            self.revert(delta);
        }
    }

    fn revert(&mut self, delta: ChangeDelta) {
        for (block, rows) in delta.added_deletes.blocks() {
            for row in rows {
                self.deletes.remove(block, *row);
            }
        }
        let keep = self
            .delete_wheres
            .len()
            .saturating_sub(delta.delete_wheres_added);
        self.delete_wheres.truncate(keep);

        for ((block, row, col), prior) in delta.overwritten_cells {
            match prior {
                Some(value) => {
                    self.updates
                        .entry(block)
                        .or_default()
                        .entry(row)
                        .or_default()
                        .insert(col, value);
                }
                None => {
                    let mut block_empty = false;
                    if let Some(rows) = self.updates.get_mut(&block) {
                        if let Some(cells) = rows.get_mut(&row) {
                            cells.remove(&col);
                            if cells.is_empty() {
                                rows.remove(&row);
                            }
                        }
                        block_empty = rows.is_empty();
                    }
                    if block_empty {
                        self.updates.remove(&block);
                    }
                }
            }
        }
        let keep = self
            .update_wheres
            .len()
            .saturating_sub(delta.update_wheres_added);
        self.update_wheres.truncate(keep);
    }
}

/// Rows matched in one evaluated result batch: `(result_row, block_row_number)`.
type Matches = [(usize, u32)];

impl BundleBuilder {
    /// Evaluate an UPDATE: find live rows matching `where_clause`, evaluate the
    /// SET expressions against their current (overlay-applied) values and store
    /// the results in the pending updates. Returns the number of rows updated.
    ///
    /// `columns` and `expressions` are parallel internal-name vectors:
    /// `columns[i]` gets value `expressions[i]`. Already-deleted rows are never
    /// matched.
    pub async fn evaluate_update_cols(
        &self,
        columns: &[String],
        expressions: &[String],
        where_clause: &str,
    ) -> Result<usize, BundlebaseError> {
        self.evaluate_update_cols_impl(columns, expressions, where_clause, None)
            .await
    }

    /// Like [`evaluate_update_cols`](Self::evaluate_update_cols) but restricted
    /// to the given base-pack block positions (resolved immediately). Used by
    /// ATTACH to re-apply always-update rules to the NEWLY attached block(s)
    /// only — re-running a non-idempotent `SET x = x + 1` rule against existing
    /// blocks would corrupt their already-updated values (bb-c6l).
    pub async fn evaluate_update_cols_in_blocks(
        &self,
        columns: &[String],
        expressions: &[String],
        where_clause: &str,
        block_indices: &[usize],
    ) -> Result<usize, BundlebaseError> {
        self.evaluate_update_cols_impl(columns, expressions, where_clause, Some(block_indices))
            .await
    }

    async fn evaluate_update_cols_impl(
        &self,
        columns: &[String],
        expressions: &[String],
        where_clause: &str,
        block_filter: Option<&[usize]>,
    ) -> Result<usize, BundlebaseError> {
        let col_ids: Vec<(String, ColumnId)> = columns
            .iter()
            .map(|internal_name| {
                let col_id =
                    bundle_schema::parse_internal_name(internal_name).ok_or_else(|| {
                        BundlebaseError::from(format!(
                            "Column '{}' is not a valid internal name",
                            internal_name
                        ))
                    })?;
                Ok((internal_name.clone(), col_id))
            })
            .collect::<Result<Vec<_>, BundlebaseError>>()?;

        let select_list: String = columns
            .iter()
            .zip(expressions.iter())
            .map(|(col, expr)| format!(", {} AS {}", expr, col))
            .collect();

        let mut updated_count = 0usize;
        for block in self.base_blocks(block_filter) {
            let key = VersionedBlockId::new(*block.id(), block.version());
            self.eval_on_block(
                &block,
                &select_list,
                where_clause,
                |input_schema, result, matches| {
                    // Coerce to the BASE column types: `SET salary = 999`
                    // yields Int64 against a Float64 column, `SET x = NULL`
                    // an untyped Null — the stored overlay must match the
                    // base data.
                    let target_types: Vec<Option<arrow::datatypes::DataType>> = col_ids
                        .iter()
                        .map(|(name, _)| {
                            input_schema
                                .field_with_name(name)
                                .ok()
                                .map(|f| f.data_type().clone())
                        })
                        .collect();
                    let mut edits = self.edits.write();
                    for &(result_row, row_number) in matches {
                        for (col_idx, (name, col_id)) in col_ids.iter().enumerate() {
                            // Column col_idx + 1: column 0 is `_idx`.
                            let value =
                                ScalarValue::try_from_array(result.column(col_idx + 1), result_row)
                                    .map_err(|e| BundlebaseError::from(e.to_string()))?;
                            let value = match &target_types[col_idx] {
                                Some(t) if &value.data_type() != t => {
                                    value.cast_to(t).map_err(|e| {
                                        BundlebaseError::from(format!(
                                            "UPDATE value for column '{}' is not castable \
                                             to the column type {:?}: {}",
                                            name, t, e
                                        ))
                                    })?
                                }
                                _ => value,
                            };
                            edits.set_cell(&key, row_number, *col_id, value);
                        }
                        updated_count += 1;
                    }
                    Ok(())
                },
            )
            .await?;
        }

        if updated_count > 0 {
            self.edits.write().push_update_where(where_clause);
        }
        Ok(updated_count)
    }

    /// Collect the live rows matching a WHERE clause (internal names) across
    /// all base-pack blocks, evaluated against current (overlay-applied)
    /// values. Already-deleted rows are never re-selected (bb-nt8).
    pub async fn select_row_ids(
        &self,
        where_clause: &str,
    ) -> Result<RowSelection, BundlebaseError> {
        self.select_row_ids_impl(where_clause, None).await
    }

    /// Like [`select_row_ids`](Self::select_row_ids) but restricted to the
    /// given base-pack block positions (resolved immediately). Used by ATTACH
    /// to re-apply always-delete rules to the NEWLY attached block(s) only
    /// (bb-c6l).
    pub async fn select_row_ids_in_blocks(
        &self,
        where_clause: &str,
        block_indices: &[usize],
    ) -> Result<RowSelection, BundlebaseError> {
        self.select_row_ids_impl(where_clause, Some(block_indices))
            .await
    }

    async fn select_row_ids_impl(
        &self,
        where_clause: &str,
        block_filter: Option<&[usize]>,
    ) -> Result<RowSelection, BundlebaseError> {
        let mut selection = RowSelection::default();
        for block in self.base_blocks(block_filter) {
            let key = VersionedBlockId::new(*block.id(), block.version());
            self.eval_on_block(&block, "", where_clause, |_, _, matches| {
                for &(_, row_number) in matches {
                    selection.insert(&key, row_number);
                }
                Ok(())
            })
            .await?;
        }
        Ok(selection)
    }

    /// Tombstone the selected rows: record them for the commit's tombstone
    /// file and mirror them into the blocks for in-session exclusion.
    pub fn mark_deleted(&self, rows: RowSelection, where_clause: &str) {
        {
            let mut edits = self.edits.write();
            for (block, block_rows) in rows.blocks() {
                for row in block_rows {
                    edits.add_delete(block, *row);
                }
            }
            edits.push_delete_where(where_clause);
        }
        self.flush_pending_deletes_to_blocks();
    }

    /// Push pending updates to the blocks for in-session visibility. Each
    /// block's session overlay is rebuilt from the FULL pending accumulator
    /// and replaces the previous flush (bb-plf). Updates for a block version
    /// no longer in the base pack (detached/replaced) are not applied.
    pub fn flush_pending_updates_to_blocks(&self) -> Result<(), BundlebaseError> {
        {
            let edits = self.edits.read();
            if edits.updates.is_empty() {
                return Ok(());
            }
            debug!(
                "[UPDATE] Flushing pending updates for {} block(s)",
                edits.updates.len()
            );
            for block in self.base_blocks(None) {
                let key = VersionedBlockId::new(*block.id(), block.version());
                if let Some(rows) = edits.updates.get(&key) {
                    block.set_session_overlay(UpdateOverlay::from_pending(rows)?);
                }
            }
        }
        self.bundle.dataframe.clear();
        self.flushed_block_state
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Push pending deleted rows to the blocks for in-session visibility.
    /// Idempotent: `add_deleted_rows` sorts and dedups.
    pub fn flush_pending_deletes_to_blocks(&self) {
        {
            let edits = self.edits.read();
            if edits.deletes.is_empty() {
                return;
            }
            debug!("[DELETE] Flushing {} pending deletes", edits.deletes.len());
            for block in self.base_blocks(None) {
                let key = VersionedBlockId::new(*block.id(), block.version());
                if let Some(rows) = edits.deletes.rows_for(&key) {
                    block.add_deleted_rows(rows.iter().copied());
                }
            }
        }
        self.bundle.dataframe.clear();
        self.flushed_block_state
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Number of blocks currently in the base pack. ATTACH captures this
    /// before and after adding data so always-rules can be scoped to just the
    /// newly attached block positions (bb-c6l).
    pub fn base_pack_block_count(&self) -> usize {
        self.base_blocks(None).len()
    }

    /// Drop the in-session flush overlay from every block (bb-plf). Used
    /// after a successful commit, where the committed `UpdateDataOp` overlay
    /// now durably represents these updates.
    pub(super) fn clear_session_overlays(&self) {
        let packs = self.bundle.packs().read().clone();
        for pack in packs.values() {
            for block in pack.blocks().iter() {
                block.clear_session_overlay();
            }
        }
    }

    /// Write the pending tombstone / update overlay files and return the
    /// `DeleteOp` / `UpdateDataOp` that record them. Edits for block
    /// versions no longer in the base pack (detached or replaced since the
    /// edit) are dropped: their row numbers don't describe any live data.
    pub(super) async fn write_pending_edit_files(
        &self,
    ) -> Result<Vec<AnyOperation>, BundlebaseError> {
        let active: HashSet<VersionedBlockId> = self
            .base_blocks(None)
            .iter()
            .map(|b| VersionedBlockId::new(*b.id(), b.version()))
            .collect();
        let (mut deletes, delete_wheres, mut updates, update_wheres) = {
            let edits = self.edits.read();
            (
                edits.deletes.clone(),
                edits.delete_wheres.clone(),
                edits.updates.clone(),
                edits.update_wheres.clone(),
            )
        };
        deletes.retain_blocks(|b| active.contains(b));
        updates.retain(|b, _| active.contains(b));

        let mut ops = Vec::new();
        let data_dir = self.bundle.data_dir();

        if !deletes.is_empty() {
            let locations = self.bundle.build_block_location_map();
            let bytes = tombstone::serialize(&deletes, &locations)?;
            debug!("[DELETE] Writing tombstone file ({} bytes)", bytes.len());
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(bytes)]);
            let address = bundlebase_common::ContentAddress::with_sub_type(
                bundlebase_common::ContentCategory::Overlay,
                "tomb",
                bundlebase_common::ContentFormat::Puffin,
            )?;
            let written = data_dir.write_stream(Box::pin(stream), &address).await?;
            let filename = data_dir.relative_path(written.file.as_ref())?;
            ops.push(AnyOperation::Delete(DeleteOp::new(
                &filename,
                delete_wheres.join("; "),
            )));
        }

        if !updates.is_empty() {
            let schema = self.bundle.schema().await?;
            let names = bundle_schema::BundleSchema::resolved(&self.operations());
            let mut column_types: HashMap<ColumnId, arrow::datatypes::DataType> = HashMap::new();
            for (col_id, real_name) in names.columns() {
                if let Some((_, field)) = schema.column_with_name(real_name) {
                    column_types.insert(*col_id, field.data_type().clone());
                }
            }
            let bytes = update_overlay::write_overlay_parquet(&updates, &column_types)?;
            debug!("[UPDATE] Writing overlay file ({} bytes)", bytes.len());
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(bytes)]);
            let address = bundlebase_common::ContentAddress::with_sub_type(
                bundlebase_common::ContentCategory::Overlay,
                "update",
                bundlebase_common::ContentFormat::Parquet,
            )?;
            let written = data_dir.write_stream(Box::pin(stream), &address).await?;
            let filename = data_dir.relative_path(written.file.as_ref())?;
            ops.push(AnyOperation::UpdateData(UpdateDataOp::new(
                &filename,
                update_wheres.join("; "),
            )));
        }

        Ok(ops)
    }

    /// Base-pack blocks, optionally restricted to the given positions.
    fn base_blocks(&self, positions: Option<&[usize]>) -> Vec<Arc<DataBlock>> {
        let blocks = self
            .bundle
            .packs()
            .read()
            .get(&ObjectId::BASE_PACK)
            .map(|pack| pack.blocks())
            .unwrap_or_default();
        match positions {
            None => blocks,
            Some(positions) => blocks
                .into_iter()
                .enumerate()
                .filter(|(idx, _)| positions.contains(idx))
                .map(|(_, block)| block)
                .collect(),
        }
    }

    /// Map internal column names back to user-visible ones (for errors).
    fn internal_to_display_names(&self) -> HashMap<String, String> {
        self.bundle_schema()
            .name_to_internal_name_map()
            .into_iter()
            .map(|(name, internal)| (internal, name))
            .collect()
    }

    /// Shared WHERE-evaluation core of DELETE and UPDATE.
    ///
    /// Streams `block`'s raw rows, renames them to internal column names and
    /// applies the block's update overlays (positions untouched), then runs
    /// `SELECT _idx<select_list> ... WHERE <where_clause>` over each batch.
    /// `on_match` receives the evaluated batch's schema, each result batch and
    /// its LIVE matches — already-tombstoned rows are filtered out here.
    ///
    /// The WHERE plan gets the same text-vs-number ordering check as
    /// `query()`, so `DELETE WHERE text_col > 100` fails instead of silently
    /// comparing lexicographically.
    async fn eval_on_block<F>(
        &self,
        block: &DataBlock,
        select_list: &str,
        where_clause: &str,
        mut on_match: F,
    ) -> Result<(), BundlebaseError>
    where
        F: FnMut(&SchemaRef, &RecordBatch, &Matches) -> Result<(), BundlebaseError>,
    {
        let deleted = block.deleted_rows();
        let overlays = block.update_overlays();
        let column_ids = block.column_ids().to_vec();
        let display_names = self.internal_to_display_names();
        let sql = format!(
            "SELECT CAST(_idx AS BIGINT) AS _idx{} FROM \
             (SELECT *, ROW_NUMBER() OVER () - 1 AS _idx FROM __edit_batch) WHERE {}",
            select_list, where_clause
        );

        // The block_ref alias is irrelevant here: only row numbers are used.
        let mut stream = block
            .reader()
            .extract_rowids_stream(ObjectIdAlias::from(0u16), self.bundle.ctx(), None)
            .await?;
        while let Some(batch_result) = stream.next().await {
            let rowid_batch = batch_result?;
            let row_numbers: Vec<u32> =
                rowid_batch.row_ids.iter().map(|r| r.row_number()).collect();
            let batch = DataBlock::rename_batch_to_internal_names(&rowid_batch.batch, &column_ids)?;
            let batch =
                UpdateOverlay::apply_to_batch(&overlays, &batch, &column_ids, &row_numbers)?;
            let input_schema = batch.schema();

            let mut config = SessionConfig::new();
            config.options_mut().sql_parser.enable_ident_normalization = false;
            let temp_ctx =
                SessionContext::new_with_config_rt(config, self.bundle.ctx().runtime_env());
            temp_ctx.register_batch("__edit_batch", batch)?;

            let df = temp_ctx.sql(&sql).await.map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to evaluate WHERE clause '{}' on block {}: {}",
                    self.bundle_schema().display_sql(where_clause),
                    block.id(),
                    e
                ))
            })?;
            crate::bundle::check_text_ordering_comparison_named(df.logical_plan(), &display_names)?;

            let mut results = df.execute_stream().await?;
            while let Some(result) = results.next().await {
                let result = result?;
                let idx_col = result
                    .column(0)
                    .as_any()
                    .downcast_ref::<arrow::array::Int64Array>()
                    .ok_or_else(|| BundlebaseError::from("Expected Int64 _idx column"))?;
                let matches: Vec<(usize, u32)> = (0..result.num_rows())
                    .filter_map(|i| {
                        let batch_idx = usize::try_from(idx_col.value(i)).ok()?;
                        let row_number = *row_numbers.get(batch_idx)?;
                        deleted
                            .binary_search(&row_number)
                            .is_err()
                            .then_some((i, row_number))
                    })
                    .collect();
                if !matches.is_empty() {
                    on_match(&input_schema, &result, &matches)?;
                }
            }
        }
        Ok(())
    }
}
