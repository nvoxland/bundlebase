use crate::DataContext;
use crate::plugin::{CsvPlugin, JsonlPlugin, ParquetPlugin, TsvPlugin, ReaderPlugin};
use crate::{BlockId, DataReader};
use bundlebase_io::DataStorage;
use bundlebase_common::BundlebaseError;
use crate::attach_format::AttachFormat;
use arrow_schema::SchemaRef;
use datafusion::common::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DataReaderFactory {
    plugins: Vec<Arc<dyn ReaderPlugin>>,
    storage: Arc<DataStorage>,
}

impl DataReaderFactory {
    pub fn new(
        storage: Arc<DataStorage>,
    ) -> Self {
        Self {
            storage: storage.clone(),
            plugins: vec![
                Arc::new(CsvPlugin::default()),
                Arc::new(TsvPlugin::default()),
                Arc::new(JsonlPlugin::default()),
                Arc::new(ParquetPlugin::default()),
            ],
        }
    }

    pub fn new_with_plugins(
        storage: Arc<DataStorage>,
        plugins: Vec<Arc<dyn ReaderPlugin>>,
    ) -> Self {
        Self {
            storage,
            plugins,
        }
    }

    pub fn storage(&self) -> &Arc<DataStorage> {
        &self.storage
    }

    /// Detect the format and create a reader by probing the file.
    ///
    /// Each plugin checks if it can handle the source (by extension and/or content
    /// validation). The first plugin that accepts returns a reader whose `format()`
    /// method indicates the detected AttachFormat.
    pub async fn detect(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
    ) -> Result<Arc<dyn DataReader>, BundlebaseError> {
        for plugin in &self.plugins {
            if let Some(reader) = plugin
                .reader(source, block_id, bundle, None, None, None, None)
                .await?
            {
                return Ok(reader);
            }
        }
        Err(DataFusionError::NotImplemented(format!(
            "No reader found for '{}'. Supported formats: .csv, .tsv, .jsonl, .parquet",
            source
        )).into())
    }

    /// Create a reader for a known format (used when re-reading existing blocks).
    ///
    /// The `format` parameter selects the reader plugin directly.
    pub async fn reader(
        &self,
        source: &str,
        format: &AttachFormat,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        read_options: Option<&HashMap<String, String>>,
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
                    read_options,
                )
                .await?
            {
                if reader.format() == *format {
                    return Ok(reader);
                }
            }
        }
        Err(DataFusionError::NotImplemented(format!("No reader found for {} (format: {})", source, format)).into())
    }
}
