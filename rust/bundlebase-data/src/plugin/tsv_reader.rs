//! TSV (tab-separated values) reader plugin.
//!
//! Reuses the CSV reader infrastructure with a tab delimiter.

use crate::DataContext;
use crate::plugin::csv_reader::{CsvFormatConfig, CsvReader};
use crate::plugin::file_reader::FilePlugin;
use crate::plugin::ReaderPlugin;
use crate::{BlockId, DataReader};
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_common::BundlebaseError;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// TSV plugin — tab-separated values. Reuses CSV reader logic with tab delimiter.
pub struct TsvPlugin {
    config: CsvFormatConfig,
}

impl Default for TsvPlugin {
    fn default() -> Self {
        Self {
            config: CsvFormatConfig::tsv(),
        }
    }
}

#[async_trait]
impl ReaderPlugin for TsvPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        read_options: Option<&HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        let lower = source.to_lowercase();
        if !lower.ends_with(".tsv") {
            return Ok(None);
        }

        let config = match read_options {
            Some(opts) if !opts.is_empty() => CsvFormatConfig::from_read_options(opts, b'\t'),
            _ => self.config.clone(),
        };
        let plugin = FilePlugin::new(config);

        let reader = plugin
            .reader(source, bundle, schema, expected_version)
            .await?;
        let layout = match layout {
            None => None,
            Some(x) => Some(ObjectStoreFile::from_str(
                x.as_str(),
                bundle.data_context_dir().as_ref(),
                bundle.config_provider(),
            )?),
        };
        Ok(Some(Arc::new(CsvReader::new(reader, block_id, &layout, crate::attach_format::AttachFormat::Tsv))))
    }
}
