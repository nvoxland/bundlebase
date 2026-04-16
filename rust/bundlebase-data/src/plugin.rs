pub mod csv_reader;
pub mod file_reader;
mod jsonl_reader;
mod parquet_reader;
mod tsv_reader;

use crate::DataReader;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
pub use csv_reader::CsvPlugin;
pub use jsonl_reader::JsonlPlugin;
pub use parquet_reader::ParquetPlugin;
use std::sync::Arc;
pub use tsv_reader::TsvPlugin;

use crate::DataContext;
use bundlebase_common::object_id::BlockId;
use bundlebase_common::BundlebaseError;

use std::collections::HashMap;

#[async_trait]
pub trait ReaderPlugin: Send + Sync {
    /// Try to create a reader for the given source.
    ///
    /// The plugin decides whether it can handle the source by checking the file
    /// extension and optionally validating content (magic bytes, format structure).
    /// Returns `Some(reader)` if this plugin handles the source, `None` otherwise.
    /// The returned reader's `format()` method indicates the detected AttachFormat.
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
