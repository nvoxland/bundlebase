mod csv_reader;
mod file_reader;
mod function_reader;
mod json_reader;
mod parquet_reader;

#[cfg(test)]
mod mock;

use crate::data::DataReader;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
pub use csv_reader::CsvPlugin;
pub use function_reader::DataGenerator;
pub use function_reader::FunctionPlugin;
pub use json_reader::JsonPlugin;
pub use parquet_reader::ParquetPlugin;
use std::sync::Arc;

#[cfg(test)]
pub use mock::MockReader;

use crate::bundle::BundleFacade;
use crate::object_id::ObjectId;
use crate::BundlebaseError;

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
    async fn reader(
        &self,
        source: &str,
        block_id: &ObjectId,
        bundle: &dyn BundleFacade,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError>;
}
