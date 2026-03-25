mod csv_reader;
pub mod file_reader;
mod json_reader;
mod parquet_reader;

use crate::DataReader;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
pub use csv_reader::CsvPlugin;
pub use json_reader::JsonPlugin;
pub use parquet_reader::ParquetPlugin;
use std::sync::Arc;

use crate::DataContext;
use bundlebase_common::object_id::BlockId;
use bundlebase_common::BundlebaseError;

use std::collections::HashMap;

#[async_trait]
pub trait ReaderPlugin: Send + Sync {
    /// Create a reader for the given source.
    ///
    /// # Arguments
    /// * `source` - URL or path to the data source
    /// * `block_id` - ID of the block being read
    /// * `bundle` - Bundle context (as trait object for flexibility)
    /// * `schema` - Optional schema (if already known)
    /// * `layout` - Optional layout file path
    /// * `expected_version` - If provided, validates version on first data access
    /// * `read_options` - Format-specific options detected at attach time (e.g., CSV newlines_in_values)
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        read_options: Option<&HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError>;
}
