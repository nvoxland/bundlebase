//! Streaming update overlay filter that merges updated values into base data.
//!
//! Wraps an inner `DataSource` and replaces cell values where the overlay
//! has updates for matching RowIds.

use crate::bundle::update_overlay::UpdateOverlay;
use crate::object_id::ColumnId;
use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use datafusion::common::Statistics;
use datafusion::datasource::source::DataSource;
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::DisplayFormatType;
use datafusion::scalar::ScalarValue;
use futures::stream::Stream;
use std::any::Any;
use std::collections::HashMap;
use std::fmt::{self, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Pre-merged overlay: row_number → (schema_column_index → ScalarValue).
/// Built at scan setup time from all overlays for a specific block.
type MergedOverlay = HashMap<u32, HashMap<usize, ScalarValue>>;

/// A DataSource wrapper that merges update overlay values into base data.
#[derive(Debug, Clone)]
pub struct UpdateOverlayDataSource {
    inner: Arc<dyn DataSource>,
    /// Pre-merged overlay keyed by row_number → (column_index → value)
    overlay: Arc<MergedOverlay>,
    updated_row_count: usize,
}

impl UpdateOverlayDataSource {
    /// Create a new overlay data source.
    ///
    /// `overlays` are pre-filtered for this block (distributed by UpdateDataOp.apply()).
    /// Later overlays override earlier ones per-cell.
    /// `column_ids` maps schema column positions to ColumnIds.
    /// `schema` is the output schema for column index resolution.
    pub fn new(
        inner: Arc<dyn DataSource>,
        overlays: &[UpdateOverlay],
        column_ids: &[ColumnId],
        schema: &SchemaRef,
    ) -> Self {
        // Pre-merge: combine all overlays, later wins per-cell
        // Convert ColumnId → schema column index
        let col_id_to_idx: HashMap<ColumnId, usize> = column_ids.iter().enumerate()
            .filter_map(|(i, cid)| {
                if i < schema.fields().len() {
                    Some((*cid, i))
                } else {
                    None
                }
            })
            .collect();

        let mut merged: MergedOverlay = HashMap::new();
        for overlay in overlays {
            for (row_id, cell_updates) in &overlay.updates {
                let row_num = row_id.row_number();
                let entry = merged.entry(row_num).or_default();
                for (col_id, value) in cell_updates {
                    if let Some(&col_idx) = col_id_to_idx.get(col_id) {
                        entry.insert(col_idx, value.clone());
                    }
                }
            }
        }

        let updated_row_count = merged.len();

        Self {
            inner,
            overlay: Arc::new(merged),
            updated_row_count,
        }
    }

    pub fn has_updates(&self) -> bool {
        self.updated_row_count > 0
    }
}

impl fmt::Display for UpdateOverlayDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "UpdateOverlay({} rows)", self.updated_row_count)
    }
}

impl DataSource for UpdateOverlayDataSource {
    fn open(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let inner_stream = self.inner.open(partition, context)?;
        let schema = inner_stream.schema();
        Ok(Box::pin(UpdateOverlayStream {
            inner: inner_stream,
            overlay: self.overlay.clone(),
            row_offset: 0,
            schema,
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> fmt::Result {
        write!(f, "UpdateOverlayDataSource({} rows)", self.updated_row_count)
    }

    fn output_partitioning(&self) -> Partitioning {
        self.inner.output_partitioning()
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        self.inner.eq_properties()
    }

    fn partition_statistics(
        &self,
        partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        self.inner.partition_statistics(partition)
    }

    fn fetch(&self) -> Option<usize> {
        self.inner.fetch()
    }

    fn with_fetch(&self, limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        self.inner.with_fetch(limit).map(|inner| {
            Arc::new(UpdateOverlayDataSource {
                inner,
                overlay: self.overlay.clone(),
                updated_row_count: self.updated_row_count,
            }) as Arc<dyn DataSource>
        })
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        Ok(None)
    }
}

/// Stream adapter that merges overlay values into base batches.
struct UpdateOverlayStream {
    inner: SendableRecordBatchStream,
    overlay: Arc<MergedOverlay>,
    row_offset: u32,
    schema: SchemaRef,
}

impl UpdateOverlayStream {
    fn apply_overlay(&self, batch: &RecordBatch, offset: u32) -> datafusion::common::Result<RecordBatch> {
        let num_rows = batch.num_rows() as u32;

        // Check if any rows in this batch have updates
        let has_updates = (offset..offset + num_rows)
            .any(|row| self.overlay.contains_key(&row));

        if !has_updates {
            return Ok(batch.clone());
        }

        // Build replacement columns
        let mut new_columns: Vec<arrow::array::ArrayRef> = Vec::with_capacity(batch.num_columns());

        for col_idx in 0..batch.num_columns() {
            let base_col = batch.column(col_idx);

            // Check if any row in this batch updates this column
            let col_has_updates = (offset..offset + num_rows)
                .any(|row| {
                    self.overlay.get(&row)
                        .map_or(false, |updates| updates.contains_key(&col_idx))
                });

            if !col_has_updates {
                new_columns.push(base_col.clone());
                continue;
            }

            // Build replacement array: for each row, use overlay value or base value
            let target_type = base_col.data_type();
            let mut scalars: Vec<ScalarValue> = Vec::with_capacity(num_rows as usize);
            for i in 0..num_rows {
                let row_num = offset + i;
                if let Some(updates) = self.overlay.get(&row_num) {
                    if let Some(value) = updates.get(&col_idx) {
                        // Cast overlay value to match base column type if needed
                        let cast_value = if value.is_null() {
                            // Create a typed null matching the base column
                            ScalarValue::try_from(target_type)
                                .map_err(|e| datafusion::common::DataFusionError::Internal(
                                    format!("Failed to create typed null for {:?}: {}", target_type, e)
                                ))?
                        } else if value.data_type() == *target_type {
                            value.clone()
                        } else {
                            value.cast_to(target_type)
                                .map_err(|e| datafusion::common::DataFusionError::Internal(
                                    format!("Failed to cast overlay value from {:?} to {:?}: {}",
                                        value.data_type(), target_type, e)
                                ))?
                        };
                        scalars.push(cast_value);
                        continue;
                    }
                }
                // Keep base value
                let base_value = ScalarValue::try_from_array(base_col, i as usize)
                    .map_err(|e| datafusion::common::DataFusionError::Internal(
                        format!("Failed to read base value: {}", e)
                    ))?;
                scalars.push(base_value);
            }

            let new_col = ScalarValue::iter_to_array(scalars.into_iter())
                .map_err(|e| datafusion::common::DataFusionError::Internal(
                    format!("Failed to build overlay array: {}", e)
                ))?;
            new_columns.push(new_col);
        }

        RecordBatch::try_new(batch.schema(), new_columns)
            .map_err(|e| datafusion::common::DataFusionError::ArrowError(Box::new(e), None))
    }
}

impl Stream for UpdateOverlayStream {
    type Item = datafusion::common::Result<RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(batch))) => {
                let num_rows = batch.num_rows() as u32;
                let offset = self.row_offset;
                self.row_offset += num_rows;

                match self.apply_overlay(&batch, offset) {
                    Ok(result) => Poll::Ready(Some(Ok(result))),
                    Err(e) => Poll::Ready(Some(Err(e))),
                }
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl RecordBatchStream for UpdateOverlayStream {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}
