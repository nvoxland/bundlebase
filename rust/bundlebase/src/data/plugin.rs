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

use crate::object_id::ObjectId;
use crate::{Bundle, BundlebaseError};

#[async_trait]
pub trait ReaderPlugin: Send + Sync {
    async fn reader(
        &self,
        source: &str,
        block_id: &ObjectId,
        bundle: &Bundle,
        schema: Option<SchemaRef>,
        layout: Option<String>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError>;
}
