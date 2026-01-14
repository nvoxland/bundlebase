mod plugin;
mod reader_factory;
mod rowid_offset_data_source;
mod rowid_provider;
mod rowid_stream;
mod versioned_blockid;

use crate::io::IOReadWriteDir;
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
pub use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;
pub use crate::object_id::ObjectId;
pub use plugin::DataGenerator;
pub use reader_factory::DataReaderFactory;
pub use crate::row_id::{RowId, RowIdBatch, SendableRowIdBatchStream};
pub use rowid_offset_data_source::{LineOrientedFormat, RowIdOffsetDataSource};
pub use rowid_provider::{LayoutRowIdProvider, RowIdProvider};
pub use rowid_stream::RowIdStreamAdapter;
use std::fmt::Debug;
use std::sync::Arc;
use url::Url;
pub use versioned_blockid::VersionedBlockId;

#[cfg(test)]
pub use plugin::MockReader;

#[async_trait]
pub trait DataReader: Sync + Send + Debug {
    fn url(&self) -> &Url;

    fn block_id(&self) -> ObjectId;

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError>;

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError>;

    async fn read_version(&self) -> Result<String, BundlebaseError>;

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError>;

    async fn build_layout(
        &self,
        _data_dir: &dyn IOReadWriteDir,
    ) -> Result<Option<Box<dyn crate::io::IOReadFile>>, BundlebaseError> {
        Ok(None)
    }

    /// Read specific rows by their RowIds
    /// Returns a stream of RecordBatches containing only the requested rows
    async fn read_rows_by_ids(
        &self,
        _row_ids: &[RowId],
        _projection: Option<&Vec<usize>>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Err("read_rows_by_ids not implemented for this adapter".into())
    }

    fn rowid_provider(&self) -> Result<Arc<dyn RowIdProvider>, BundlebaseError> {
        Err("rowid_generator not implemented for this adapter".into())
    }

    /// Stream data with RowIds for index building
    /// Each batch is paired with RowIds indicating the file position of each row
    /// Used by CreateIndexOp to build indexes that reference actual file positions
    async fn extract_rowids_stream(
        &self,
        ctx: Arc<SessionContext>,
        projection: Option<&Vec<usize>>,
    ) -> Result<SendableRowIdBatchStream, BundlebaseError> {
        let data_source = self
            .data_source(projection, &[], None, None)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        let record_batch_stream = data_source
            .open(0, ctx.task_ctx())
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        let provider = self.rowid_provider()?;

        Ok(Box::pin(RowIdStreamAdapter::new(
            record_batch_stream,
            provider,
        )))
    }
}
