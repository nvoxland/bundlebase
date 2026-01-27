use crate::bundle::BundleFacade;
use crate::data::plugin::{CsvPlugin, FunctionPlugin, JsonPlugin, ParquetPlugin, ReaderPlugin};
use crate::data::{DataReader, ObjectId};
use crate::functions::FunctionRegistry;
use crate::io::DataStorage;
use crate::BundlebaseError;
use arrow_schema::SchemaRef;
use datafusion::common::DataFusionError;
use parking_lot::RwLock;
use std::sync::Arc;

pub struct DataReaderFactory {
    plugins: Vec<Arc<dyn ReaderPlugin>>,
    storage: Arc<DataStorage>,
}

impl DataReaderFactory {
    pub fn new(
        function_registry: Arc<RwLock<FunctionRegistry>>,
        storage: Arc<DataStorage>,
    ) -> Self {
        Self {
            storage: storage.clone(),
            plugins: vec![
                Arc::new(CsvPlugin::default()),
                Arc::new(FunctionPlugin::new(function_registry.clone())),
                Arc::new(JsonPlugin::default()),
                Arc::new(ParquetPlugin::default()),
            ],
        }
    }

    pub fn storage(&self) -> &Arc<DataStorage> {
        &self.storage
    }

    /// Create a reader for the given source.
    ///
    /// # Arguments
    /// * `source` - URL or path to the data source
    /// * `block_id` - ID of the block being read
    /// * `bundle` - Bundle context (as trait object for flexibility)
    /// * `schema` - Optional schema (if already known)
    /// * `layout` - Optional layout file path
    /// * `expected_version` - If provided, validates version on first data access.
    ///   This is used to detect when source files have changed since the bundle was created.
    pub async fn reader(
        &self,
        source: &str,
        block_id: &ObjectId,
        bundle: &dyn BundleFacade,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
    ) -> Result<Arc<dyn DataReader>, BundlebaseError> {
        for plugin in &self.plugins {
            if let Some(reader) = plugin
                .reader(
                    source,
                    block_id,
                    bundle,
                    schema.clone(),
                    layout.clone(),
                    expected_version.clone(),
                )
                .await?
            {
                return Ok(reader);
            }
        }
        Err(DataFusionError::NotImplemented(format!("No reader found for {}", source)).into())
    }
}
