//! Data reader trait and RowId provider trait.
//!
//! These traits define the interface for reading data from storage backends.
//! Implementations live in the core crate's data module.

use crate::{BlockId, BundlebaseError, ObjectIdAlias, RowId, SendableRowIdBatchStream};
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use url::Url;

/// Trait for providing RowIds within a specific range.
///
/// Different implementations can use different strategies:
/// - Pre-loaded from a layout file with caching (CSV)
/// - Computed on-the-fly based on file metadata (Parquet)
#[async_trait]
pub trait RowIdProvider: Send + Sync {
    /// Generate RowIds for rows in the range [begin, end)
    async fn get_row_ids(&self, begin: usize, end: usize) -> Result<Vec<RowId>, BundlebaseError>;
}

/// Trait for reading data from a storage backend.
///
/// Each data format (CSV, Parquet, JSON) implements this trait to provide
/// schema inference, data reading, and row-level access.
#[async_trait]
pub trait DataReader: Sync + Send + Debug {
    /// URL of the underlying data file.
    fn url(&self) -> &Url;

    /// Block ID this reader is associated with.
    fn block_id(&self) -> BlockId;

    /// Read the schema of the data.
    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError>;

    /// Read statistics about the data (row count, column stats, etc.)
    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError>;

    /// Read the version of the data file (ETag, modification time, etc.)
    async fn read_version(&self) -> Result<String, BundlebaseError>;

    /// Create a DataFusion DataSource for query execution.
    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError>;

    /// Build a layout file for RowId-based access.
    /// Returns None if this format doesn't need a layout file.
    async fn build_layout(
        &self,
        _data_dir: &dyn crate::io_dir::IOReadWriteDir,
    ) -> Result<Option<Box<dyn crate::io_file::IOReadFile>>, BundlebaseError> {
        Ok(None)
    }

    /// Read specific rows by their RowIds.
    async fn read_rows_by_ids(
        &self,
        _row_ids: &[RowId],
        _projection: Option<&Vec<usize>>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Err("read_rows_by_ids not implemented for this adapter".into())
    }

    /// Get a RowId provider for this data reader.
    fn rowid_provider(&self) -> Result<Arc<dyn RowIdProvider>, BundlebaseError> {
        Err("rowid_generator not implemented for this adapter".into())
    }

    /// Return format-specific options detected during schema inference.
    fn read_options(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    /// Stream data with RowIds for index building.
    async fn extract_rowids_stream(
        &self,
        block_ref: ObjectIdAlias,
        ctx: Arc<SessionContext>,
        projection: Option<&Vec<usize>>,
    ) -> Result<SendableRowIdBatchStream, BundlebaseError>;
}
