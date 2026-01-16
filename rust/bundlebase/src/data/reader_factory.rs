use crate::data::plugin::{CsvPlugin, FunctionPlugin, JsonPlugin, ParquetPlugin, ReaderPlugin};
use crate::data::{DataReader, ObjectId};
use crate::functions::FunctionRegistry;
use crate::io::DataStorage;
use crate::{Bundle, BundlebaseError};
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

    pub async fn reader(
        &self,
        source: &str,
        block_id: &ObjectId,
        bundle: &Bundle,
        schema: Option<SchemaRef>,
        layout: Option<String>,
    ) -> Result<Arc<dyn DataReader>, BundlebaseError> {
        for plugin in &self.plugins {
            if let Some(reader) = plugin
                .reader(source, block_id, bundle, schema.clone(), layout.clone())
                .await?
            {
                return Ok(reader);
            }
        }
        Err(DataFusionError::NotImplemented(format!("No reader found for {}", source)).into())
    }
}
