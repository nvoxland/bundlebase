use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use bundlebase::bundle::{DataBlock, Pack};
use bundlebase_common::arrow_types::widen_type;
use bundlebase_common::object_id::ObjectId;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::TableType;
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_expr::expressions::{CastExpr, Column, Literal};
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::physical_plan::{union::UnionExec, ExecutionPlan};
use datafusion::scalar::ScalarValue;
use futures::future::try_join_all;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// Custom TableProvider that represents a UNION of all blocks in a pack.
///
/// This table lazily constructs the UNION when scanned, maintaining the streaming
/// execution model. Multiple blocks in a pack are combined using UNION BY NAME.
/// Blocks with different column sets are handled by inserting null columns where needed.
pub struct PackTable {
    pack_id: ObjectId,
    pack: Arc<Pack>,
    schema: SchemaRef,
}

impl std::fmt::Debug for PackTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackUnionTable")
            .field("pack_id", &self.pack_id)
            .field("pack", &self.pack)
            .field("schema", &self.schema)
            .finish()
    }
}

impl PackTable {
    pub fn new(pack_id: ObjectId, pack: Arc<Pack>) -> Result<Self> {
        let blocks = pack.blocks();

        if blocks.is_empty() {
            return Err(datafusion::error::DataFusionError::Plan(format!(
                "Pack {} has no blocks",
                pack_id
            )));
        }

        // Compute merged schema from ALL blocks (union of fields by name).
        // Preserves insertion order: first block's fields first, then additional
        // fields from subsequent blocks appended in order. When the same name
        // appears with different types across blocks, widen to a common type
        // (e.g. Utf8 + Utf8View -> Utf8View) so the per-block plans can be
        // unioned without DataFusion failing on schema mismatch.
        let mut merged_fields: Vec<Arc<Field>> = Vec::new();
        let mut name_to_idx: HashMap<String, usize> = HashMap::new();
        for block in &blocks {
            let block_schema = block.schema();
            for field in block_schema.fields() {
                match name_to_idx.get(field.name()) {
                    None => {
                        name_to_idx.insert(field.name().clone(), merged_fields.len());
                        merged_fields.push(field.clone());
                    }
                    Some(&idx) => {
                        let existing = &merged_fields[idx];
                        if existing.data_type() != field.data_type() {
                            let widened = widen_type(existing.data_type(), field.data_type());
                            let nullable = existing.is_nullable() || field.is_nullable();
                            merged_fields[idx] = Arc::new(Field::new(
                                existing.name(),
                                widened,
                                nullable,
                            ));
                        }
                    }
                }
            }
        }

        let schema = Arc::new(Schema::new(merged_fields));

        Ok(Self {
            pack_id,
            pack,
            schema,
        })
    }
}

#[async_trait]
impl TableProvider for PackTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::View
    }

    fn statistics(&self) -> Option<datafusion::common::Statistics> {
        use datafusion::common::stats::Precision;
        use datafusion::common::{ColumnStatistics, Statistics};

        let blocks = self.pack.blocks();
        if blocks.is_empty() {
            return None;
        }

        // Sum row counts across blocks. If any block lacks a row count we
        // fall back to Absent rather than reporting a partial total.
        let mut total_rows: Option<usize> = Some(0);
        for block in &blocks {
            match block.statistics().map(|s| s.num_rows) {
                Some(Precision::Exact(n)) | Some(Precision::Inexact(n)) => {
                    total_rows = total_rows.map(|t| t + n);
                }
                _ => {
                    total_rows = None;
                    break;
                }
            }
        }

        Some(Statistics {
            num_rows: total_rows
                .map(Precision::Exact)
                .unwrap_or(Precision::Absent),
            total_byte_size: Precision::Absent,
            column_statistics: self
                .schema
                .fields()
                .iter()
                .map(|_| ColumnStatistics::new_unknown())
                .collect(),
        })
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

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        log::debug!(
            "PackUnionTable.scan() called with projection: {:?}, filters: {:?}",
            projection,
            filters
        );
        let blocks = self.pack.blocks();
        let pack_schema = &self.schema;

        // Determine which pack-level column names are requested. Cloning the
        // Arc<Field>s lets us move the projected schema into the per-block
        // futures without borrowing &self across awaits.
        let projected_pack_fields: Vec<(usize, Arc<Field>)> = match projection {
            Some(proj) => proj
                .iter()
                .map(|&i| (i, pack_schema.fields()[i].clone()))
                .collect(),
            None => pack_schema
                .fields()
                .iter()
                .enumerate()
                .map(|(i, f)| (i, f.clone()))
                .collect(),
        };

        // Limit early-stop: when a row limit is in effect and we know per-block
        // row counts, only schedule plans for blocks that contribute to the
        // first `limit` rows. Conservative: a block with unknown num_rows is
        // always included.
        let blocks_to_plan: Vec<Arc<DataBlock>> = match limit {
            Some(lim) => {
                let mut acc: usize = 0;
                let mut kept: Vec<Arc<DataBlock>> = Vec::new();
                for block in &blocks {
                    if acc >= lim {
                        break;
                    }
                    kept.push(block.clone());
                    match block.num_rows() {
                        Some(n) => acc = acc.saturating_add(n),
                        None => {
                            // Unknown count: can't reason about further blocks.
                            // Include this one, then keep going (no early stop).
                            acc = lim;
                        }
                    }
                }
                kept
            }
            None => blocks.iter().cloned().collect(),
        };

        // Build per-block plans concurrently. Each future is independent —
        // they only read DataBlock state and call into the async DataReader.
        let filters_owned: Vec<Expr> = filters.to_vec();
        let pack_fields_shared = Arc::new(projected_pack_fields);
        let plan_futures =
            blocks_to_plan.into_iter().map(|block| {
                let pack_fields = Arc::clone(&pack_fields_shared);
                let filters_owned = filters_owned.clone();
                async move {
                    Self::plan_for_block(block, state, &pack_fields, &filters_owned, limit).await
                }
            });
        let inputs: Vec<Arc<dyn ExecutionPlan>> = try_join_all(plan_futures).await?;

        // If only one block, return its plan directly
        if let [plan] = inputs.as_slice() {
            return Ok(plan.clone());
        }

        // Create a UnionExec to combine all block plans
        Ok(UnionExec::try_new(inputs)?)
    }
}

impl PackTable {
    /// Build the per-block ExecutionPlan. Always wraps the block scan in a
    /// projection so each block's output schema matches the pack-level schema
    /// exactly (column order, types, missing columns padded with nulls). This
    /// is required for the UnionExec across blocks to plan successfully when
    /// blocks have heterogeneous types for the same column (e.g. Utf8 vs
    /// Utf8View) or omit columns.
    async fn plan_for_block(
        block: Arc<DataBlock>,
        state: &dyn Session,
        projected_pack_fields: &[(usize, Arc<Field>)],
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let block_schema = block.schema();

        // Indices of fields to scan from the block, in scan-output order.
        // Each entry is (block_field_idx, position-in-projected_pack_fields).
        let mut scan_entries: Vec<(usize, usize)> = Vec::new();
        for (output_idx, (_pack_idx, pack_field)) in projected_pack_fields.iter().enumerate() {
            if let Some((block_idx, _)) = block_schema
                .fields()
                .iter()
                .enumerate()
                .find(|(_, f)| f.name() == pack_field.name())
            {
                scan_entries.push((block_idx, output_idx));
            }
        }

        let scan_proj: Option<Vec<usize>> = if scan_entries.is_empty() {
            None
        } else {
            Some(scan_entries.iter().map(|(b, _)| *b).collect())
        };
        let inner_plan = block
            .scan(state, scan_proj.as_ref(), filters, limit)
            .await?;
        let inner_schema = inner_plan.schema();

        // Map output_idx -> (inner_col_idx, inner_data_type).
        let mut output_to_inner: HashMap<usize, (usize, DataType)> = HashMap::new();
        for (i, (_block_idx, output_idx)) in scan_entries.iter().enumerate() {
            let dt = inner_schema.fields()[i].data_type().clone();
            output_to_inner.insert(*output_idx, (i, dt));
        }

        let mut exprs: Vec<(Arc<dyn PhysicalExpr>, String)> =
            Vec::with_capacity(projected_pack_fields.len());
        for (output_idx, (_pack_idx, pack_field)) in projected_pack_fields.iter().enumerate() {
            match output_to_inner.get(&output_idx) {
                Some((inner_col_idx, inner_dt)) => {
                    let inner_name = inner_schema.fields()[*inner_col_idx].name();
                    let mut e: Arc<dyn PhysicalExpr> =
                        Arc::new(Column::new(inner_name, *inner_col_idx));
                    if inner_dt != pack_field.data_type() {
                        e = Arc::new(CastExpr::new(e, pack_field.data_type().clone(), None));
                    }
                    exprs.push((e, pack_field.name().clone()));
                }
                None => {
                    let null_value = ScalarValue::try_from(pack_field.data_type())?;
                    exprs.push((
                        Arc::new(Literal::new(null_value)),
                        pack_field.name().clone(),
                    ));
                }
            }
        }

        Ok(Arc::new(ProjectionExec::try_new(exprs, inner_plan)?))
    }
}

// Unit tests are covered by integration tests (basic_e2e, source_e2e).
// `widen_type` lives in bundlebase-common::arrow_types for reuse by the
// indexer, which faces the same per-block type-mismatch problem.
