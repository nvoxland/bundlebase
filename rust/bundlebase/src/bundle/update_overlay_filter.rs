//! Streaming update overlay filter that merges updated values into base data.
//!
//! Wraps an inner `DataSource` and replaces cell values where the overlay
//! has updates for matching row numbers. Uses Arrow arrays directly — no
//! ScalarValue allocation per cell.

use crate::bundle::update_overlay::UpdateOverlay;
use crate::object_id::ColumnId;
use arrow::array::{ArrayRef, BooleanArray, RecordBatch};
use arrow::datatypes::SchemaRef;
use datafusion::common::Statistics;
use datafusion::datasource::source::DataSource;
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::DisplayFormatType;
use futures::stream::Stream;
use std::any::Any;
use std::collections::HashMap;
use std::fmt::{self, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Pre-merged overlay in Arrow-native form.
/// row_numbers are sorted. Column arrays are indexed by schema column position.
struct MergedOverlay {
    /// Sorted row numbers that have updates
    row_numbers: Vec<u32>,
    /// For each schema column index: (values array, is_set mask).
    /// Both have the same length as row_numbers.
    /// Only columns with at least one update are present.
    columns: HashMap<usize, (ArrayRef, BooleanArray)>,
}

/// A DataSource wrapper that merges update overlay values into base data.
#[derive(Debug, Clone)]
pub struct UpdateOverlayDataSource {
    inner: Arc<dyn DataSource>,
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
        // Build ColumnId → schema column index mapping
        let col_id_to_idx: HashMap<ColumnId, usize> = column_ids.iter().enumerate()
            .filter_map(|(i, cid)| {
                if i < schema.fields().len() {
                    Some((*cid, i))
                } else {
                    None
                }
            })
            .collect();

        // Merge all overlays, converting ColumnId keys to schema column indices
        let merged_overlay = UpdateOverlay::merge(overlays);
        let mut columns: HashMap<usize, (ArrayRef, BooleanArray)> = HashMap::new();
        for (col_id, (values, is_set)) in &merged_overlay.columns {
            if let Some(&col_idx) = col_id_to_idx.get(col_id) {
                columns.insert(col_idx, (values.clone(), is_set.clone()));
            } else {
                log::warn!("Overlay column {:?} not found in block schema — skipping", col_id);
            }
        }

        let updated_row_count = merged_overlay.row_numbers.len();

        Self {
            inner,
            overlay: Arc::new(MergedOverlay {
                row_numbers: merged_overlay.row_numbers,
                columns,
            }),
            updated_row_count,
        }
    }

    pub fn has_updates(&self) -> bool {
        self.updated_row_count > 0
    }
}

impl fmt::Debug for MergedOverlay {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "MergedOverlay({} rows, {} columns)", self.row_numbers.len(), self.columns.len())
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

        if batch.num_columns() == 0 || num_rows == 0 {
            return Ok(batch.clone());
        }

        // Binary search to find overlay rows in [offset, offset + num_rows)
        let start = self.overlay.row_numbers.partition_point(|&r| r < offset);
        let end = self.overlay.row_numbers.partition_point(|&r| r < offset + num_rows);

        if start == end {
            // No overlay rows in this batch range
            return Ok(batch.clone());
        }

        // Build replacement columns
        let mut new_columns: Vec<ArrayRef> = Vec::with_capacity(batch.num_columns());

        for col_idx in 0..batch.num_columns() {
            let base_col = batch.column(col_idx);

            let overlay_col = match self.overlay.columns.get(&col_idx) {
                Some(col) => col,
                None => {
                    // No updates for this column
                    new_columns.push(base_col.clone());
                    continue;
                }
            };

            let (values, is_set) = overlay_col;

            // Check if any overlay row in range actually updates this column
            let has_updates = (start..end).any(|i| is_set.value(i));
            if !has_updates {
                new_columns.push(base_col.clone());
                continue;
            }

            // Cast overlay values to match base column type if needed
            let overlay_values: &dyn arrow::array::Array = if values.data_type() == base_col.data_type() {
                values.as_ref()
            } else {
                // Type mismatch — fall back to base column (overlay values incompatible)
                log::warn!(
                    "Overlay type mismatch for column {}: overlay={:?}, base={:?} — skipping update",
                    col_idx, values.data_type(), base_col.data_type()
                );
                new_columns.push(base_col.clone());
                continue;
            };

            // Use MutableArrayData to copy ranges from base (source 0) or overlay (source 1)
            // without per-cell ScalarValue allocation
            let base_data = base_col.to_data();
            let overlay_data = overlay_values.to_data();
            let mut builder = arrow::array::MutableArrayData::new(
                vec![&base_data, &overlay_data], false, num_rows as usize,
            );

            let mut overlay_pos = start;
            let mut base_run_start: usize = 0; // start of current contiguous base range

            for i in 0..num_rows as usize {
                let row_num = offset + i as u32;

                // Advance overlay_pos to match current row
                while overlay_pos < end && self.overlay.row_numbers[overlay_pos] < row_num {
                    overlay_pos += 1;
                }

                if overlay_pos < end
                    && self.overlay.row_numbers[overlay_pos] == row_num
                    && is_set.value(overlay_pos)
                {
                    // Flush any pending base rows
                    if base_run_start < i {
                        builder.extend(0, base_run_start, i);
                    }
                    // Copy one row from overlay (source 1)
                    builder.extend(1, overlay_pos, overlay_pos + 1);
                    base_run_start = i + 1;
                }
            }
            // Flush remaining base rows
            if base_run_start < num_rows as usize {
                builder.extend(0, base_run_start, num_rows as usize);
            }

            let new_col = arrow::array::make_array(builder.freeze());
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
