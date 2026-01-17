use crate::data::{LineOrientedFormat, RowId, RowIdOffsetDataSource};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use crate::{Bundle, BundlebaseError};
use arrow::datatypes::SchemaRef;
use datafusion::common::DataFusionError;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::listing::PartitionedFile;
use datafusion::datasource::physical_plan::{FileScanConfigBuilder, FileSource};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use datafusion::prelude::SessionContext;
use std::sync::Arc;
use url::Url;

/// Configuration for a file-based format (CSV, JSON, Parquet, etc.)
pub trait FileFormatConfig: Send + Sync + Default + Clone {
    /// File extension this format handles (e.g., ".csv")
    fn extension(&self) -> &'static str;

    /// Get the FileFormat object for schema inference
    fn file_format(&self) -> Arc<dyn FileFormat>;

    /// Get the FileSource for this format (e.g., CsvSource, JsonSource, ParquetSource)
    /// The schema is required by the FileSource constructors in DataFusion 52+
    fn file_source(&self, schema: SchemaRef) -> Arc<dyn FileSource>;

    /// Get the line-oriented format if this format supports it (CSV or JSON Lines)
    /// Returns None for formats that don't use line-based offset reading (like Parquet)
    fn line_oriented_format(&self) -> Option<LineOrientedFormat> {
        None
    }
}

/// Generic plugin for file-based data formats
/// This is a utility that plugin implementations can use
pub struct FilePlugin<C: FileFormatConfig> {
    config: C,
}

impl<C: FileFormatConfig> FilePlugin<C> {
    pub fn new(config: C) -> Self {
        Self { config }
    }

    /// Check if this plugin handles the given URL (by extension)
    pub fn handles(&self, source: &str) -> bool {
        source.ends_with(self.config.extension())
    }

    pub async fn reader(
        &self,
        source: &str,
        bundle: &Bundle,
        schema: Option<SchemaRef>,
    ) -> Result<FileReader<C>, BundlebaseError> {
        Ok(FileReader::new(
            &ObjectStoreFile::from_str(source, bundle.data_dir(), bundle.config())?,
            self.config.clone(),
            bundle.ctx(),
            schema,
        ))
    }
}

impl<C: FileFormatConfig> Default for FilePlugin<C> {
    fn default() -> Self {
        Self::new(C::default())
    }
}

pub struct FileReader<C: FileFormatConfig> {
    file: ObjectStoreFile,
    config: C,
    ctx: Arc<SessionContext>,
    schema: Option<SchemaRef>,
}

impl<C: FileFormatConfig> FileReader<C> {
    pub fn new(
        file: &ObjectStoreFile,
        config: C,
        ctx: Arc<SessionContext>,
        schema: Option<SchemaRef>,
    ) -> Self {
        Self {
            file: file.clone(),
            ctx,
            schema,
            config,
        }
    }
}

impl<C: FileFormatConfig> FileReader<C> {
    /// Get the IOFile
    pub fn file(&self) -> &ObjectStoreFile {
        &self.file
    }

    /// Get the URL of the file
    pub fn url(&self) -> &Url {
        self.file.url()
    }

    /// Get the object store
    pub fn object_store(&self) -> Arc<dyn object_store::ObjectStore> {
        self.file.store()
    }

    /// Get the schema of the file
    pub async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        let metadata = self
            .file
            .object_meta()
            .await?
            .ok_or(format!("File not found: {}", self.file.url()))?;

        Ok(Some(
            self.config
                .file_format()
                .infer_schema(&self.ctx.state(), &self.file.store(), &[metadata])
                .await?,
        ))
    }

    /// Get the version of the file (from ObjectStore metadata)
    pub async fn version(&self) -> Result<String, BundlebaseError> {
        self.file.version().await
    }

    /// Generic data_source implementation for file-based readers
    pub async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        // Return RowIdOffsetDataSource for selective row reading if format supports it
        if let Some(ids) = row_ids {
            if let Some(format) = self.config.line_oriented_format() {
                return Ok(Arc::new(RowIdOffsetDataSource::new(
                    &self.file,
                    self.schema.clone().expect("No schema set"),
                    ids.to_vec(),
                    projection.cloned(),
                    format,
                )));
            }
            // Format doesn't support line-oriented reading, fall back to full scan
            // This can happen with Parquet files
        }

        let metadata = self.file.object_meta().await?.ok_or_else(|| {
            DataFusionError::Internal(format!(
                "File metadata not available for: {}",
                self.file.url()
            ))
        })?;

        let partitioned_file = PartitionedFile::from(metadata);

        let schema = self.schema.clone().expect("No schema set");
        let mut builder = FileScanConfigBuilder::new(
            self.file.store_url(),
            self.config.file_source(schema),
        )
        .with_file(partitioned_file);

        if let Some(proj) = projection {
            builder = builder.with_projection_indices(Some(proj.to_vec()))?;
        }

        if let Some(lim) = limit {
            builder = builder.with_limit(Some(lim));
        }

        Ok(Arc::new(builder.build()))
    }
}

// impl<C: FileFormatConfig> Clone for FileReader<C> {
//     fn clone(&self) -> Self {
//         Self {
//             file: self.file.clone(),
//             schema: self.schema.clone(),
//             config: self.config.clone(),
//         }
//     }
// }

impl<C: FileFormatConfig> std::fmt::Debug for FileReader<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("FileReader")
            .field("file", &self.file)
            .finish()
    }
}
