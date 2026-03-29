//! Zero-copy DataSource wrapper that renames batch columns from physical
//! names to stable `col_<id>` names.

use crate::bundle::column_metadata;
use crate::object_id::ColumnId;
use arrow::datatypes::SchemaRef;
use datafusion::common::Statistics;
use datafusion::datasource::source::DataSource;
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::DisplayFormatType;
use futures::stream::Stream;
use std::any::Any;
use std::fmt::{self, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Wraps a DataSource and replaces field names on emitted batches
/// using stable `col_<id>` names derived from ColumnIds.
/// The column data is unchanged (zero-copy); only field names change.
#[derive(Debug, Clone)]
pub struct SchemaRenameDataSource {
    inner: Arc<dyn DataSource>,
    /// The col_<id> schema used for DataFusion planning.
    planning_schema: SchemaRef,
    /// Column IDs used to rename each batch's fields dynamically.
    column_ids: Vec<ColumnId>,
}

impl SchemaRenameDataSource {
    pub fn new(
        inner: Arc<dyn DataSource>,
        planning_schema: SchemaRef,
        column_ids: Vec<ColumnId>,
    ) -> Self {
        Self {
            inner,
            planning_schema,
            column_ids,
        }
    }
}

impl fmt::Display for SchemaRenameDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "SchemaRename")
    }
}

impl DataSource for SchemaRenameDataSource {
    fn open(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let inner_stream = self.inner.open(partition, context)?;
        Ok(Box::pin(SchemaRenameStream {
            inner: inner_stream,
            column_ids: self.column_ids.clone(),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> fmt::Result {
        write!(f, "SchemaRenameDataSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        self.inner.output_partitioning()
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        // Use the inner source's eq_properties schema, renamed with col_<id> names.
        // This ensures types match the actual batch types, not the stored schema.
        let inner_schema = self.inner.eq_properties().schema().clone();
        let id_fields: Vec<Arc<arrow::datatypes::Field>> = inner_schema
            .fields()
            .iter()
            .zip(self.column_ids.iter())
            .map(|(field, col_id)| {
                Arc::new(field.as_ref().clone().with_name(column_metadata::col_id_name(col_id)))
            })
            .collect();
        let renamed_schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
            id_fields,
            inner_schema.metadata().clone(),
        ));
        EquivalenceProperties::new(renamed_schema)
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
            Arc::new(SchemaRenameDataSource::new(
                inner,
                self.planning_schema.clone(),
                self.column_ids.clone(),
            )) as Arc<dyn DataSource>
        })
    }

    fn try_swapping_with_projection(
        &self,
        projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        self.inner.try_swapping_with_projection(projection)
    }
}

struct SchemaRenameStream {
    inner: SendableRecordBatchStream,
    column_ids: Vec<ColumnId>,
}

impl Stream for SchemaRenameStream {
    type Item = datafusion::common::Result<arrow::record_batch::RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(batch))) => {
                // Build col_<id> schema from the batch's actual field types
                let batch_schema = batch.schema();
                let id_fields: Vec<Arc<arrow::datatypes::Field>> = batch_schema
                    .fields()
                    .iter()
                    .zip(self.column_ids.iter())
                    .map(|(field, col_id)| {
                        Arc::new(field.as_ref().clone().with_name(column_metadata::col_id_name(col_id)))
                    })
                    .collect();
                let id_schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
                    id_fields,
                    batch_schema.metadata().clone(),
                ));
                let renamed = arrow::record_batch::RecordBatch::try_new(
                    id_schema,
                    batch.columns().to_vec(),
                )
                .map_err(|e| datafusion::common::DataFusionError::ArrowError(Box::new(e), None));
                Poll::Ready(Some(renamed))
            }
            other => other,
        }
    }
}

impl RecordBatchStream for SchemaRenameStream {
    fn schema(&self) -> SchemaRef {
        // Use the inner stream's schema renamed with col_<id> names
        let inner_schema = self.inner.schema();
        let id_fields: Vec<Arc<arrow::datatypes::Field>> = inner_schema
            .fields()
            .iter()
            .zip(self.column_ids.iter())
            .map(|(field, col_id)| {
                Arc::new(field.as_ref().clone().with_name(column_metadata::col_id_name(col_id)))
            })
            .collect();
        Arc::new(arrow::datatypes::Schema::new_with_metadata(
            id_fields,
            inner_schema.metadata().clone(),
        ))
    }
}
