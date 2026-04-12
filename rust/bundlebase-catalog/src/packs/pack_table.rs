use bundlebase::bundle::{DataBlock, Pack};
use bundlebase_common::object_id::ObjectId;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::TableType;
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_expr::expressions::{Column, Literal};
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::physical_plan::{union::UnionExec, ExecutionPlan};
use datafusion::scalar::ScalarValue;
use futures::future::try_join_all;
use std::any::Any;
use std::collections::HashSet;
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
        // fields from subsequent blocks appended in order.
        let mut merged_fields: Vec<Arc<Field>> = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();
        for block in &blocks {
            let block_schema = block.schema();
            for field in block_schema.fields() {
                if seen_names.insert(field.name().clone()) {
                    merged_fields.push(field.clone());
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
            num_rows: total_rows.map(Precision::Exact).unwrap_or(Precision::Absent),
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
            Some(proj) => proj.iter().map(|&i| (i, pack_schema.fields()[i].clone())).collect(),
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
        let plan_futures = blocks_to_plan.into_iter().map(|block| {
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
    /// Build the per-block ExecutionPlan, including the missing-columns
    /// padding path. Extracted from `scan()` so it can be invoked
    /// concurrently per block via `try_join_all`.
    async fn plan_for_block(
        block: Arc<DataBlock>,
        state: &dyn Session,
        projected_pack_fields: &[(usize, Arc<Field>)],
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let block_schema = block.schema();

        // Build mapping: for each projected pack column, find it in block schema.
        let mut block_proj_indices: Vec<usize> = Vec::new();
        let mut missing_columns: Vec<(usize, Arc<Field>)> = Vec::new();
        let mut all_present = true;

        for (output_idx, (_pack_idx, pack_field)) in projected_pack_fields.iter().enumerate() {
            if let Some((block_idx, _)) = block_schema
                .fields()
                .iter()
                .enumerate()
                .find(|(_, f)| f.name() == pack_field.name())
            {
                block_proj_indices.push(block_idx);
            } else {
                all_present = false;
                missing_columns.push((output_idx, pack_field.clone()));
            }
        }

        if all_present {
            // All projected columns exist in this block — pass translated projection
            let block_proj = if block_proj_indices.len() == block_schema.fields().len()
                && block_proj_indices.iter().enumerate().all(|(i, &v)| i == v)
            {
                None // identity projection, pass None for efficiency
            } else {
                Some(block_proj_indices)
            };
            block.scan(state, block_proj.as_ref(), filters, limit).await
        } else {
            // Some columns missing — scan existing columns, then project to add nulls
            let existing_proj: Vec<usize> = projected_pack_fields
                .iter()
                .filter_map(|(_, pack_field)| {
                    block_schema
                        .fields()
                        .iter()
                        .position(|f| f.name() == pack_field.name())
                })
                .collect();

            let scan_proj = if existing_proj.is_empty() {
                None
            } else {
                Some(existing_proj)
            };
            let inner_plan = block.scan(state, scan_proj.as_ref(), filters, limit).await?;
            let inner_schema = inner_plan.schema();

            let mut exprs: Vec<(Arc<dyn datafusion::physical_expr::PhysicalExpr>, String)> =
                Vec::new();
            let mut inner_col_idx = 0;

            for (output_idx, (_pack_idx, pack_field)) in projected_pack_fields.iter().enumerate() {
                if missing_columns.iter().any(|(mi, _)| *mi == output_idx) {
                    let null_value = ScalarValue::try_from(pack_field.data_type())?;
                    exprs.push((
                        Arc::new(Literal::new(null_value)),
                        pack_field.name().clone(),
                    ));
                } else {
                    let inner_field = &inner_schema.fields()[inner_col_idx];
                    exprs.push((
                        Arc::new(Column::new(inner_field.name(), inner_col_idx)),
                        pack_field.name().clone(),
                    ));
                    inner_col_idx += 1;
                }
            }

            Ok(Arc::new(ProjectionExec::try_new(exprs, inner_plan)?))
        }
    }
}

// Unit tests are covered by integration tests (basic_e2e, source_e2e)
