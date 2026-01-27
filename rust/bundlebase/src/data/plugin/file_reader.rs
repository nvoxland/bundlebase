use crate::bundle::BundleFacade;
use crate::data::{LineOrientedFormat, RowId, RowIdOffsetDataSource};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::plugin::versioned_object_store::VersionedObjectStoreFile;
use crate::io::IOReadFile;
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use datafusion::common::DataFusionError;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::listing::PartitionedFile;
use datafusion::datasource::object_store::ObjectStoreUrl;
use datafusion::datasource::physical_plan::{FileScanConfigBuilder, FileSource};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use datafusion::prelude::SessionContext;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectMeta, ObjectStore};
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

/// Either a plain ObjectStoreFile or one with version validation.
///
/// When `expected_version` is provided during reader creation, version is validated
/// on first data access. This prevents silently reading stale data when the source
/// file has changed since the bundle was created.
#[derive(Debug, Clone)]
pub enum MaybeVersionedFile {
    Plain(ObjectStoreFile),
    Versioned(VersionedObjectStoreFile),
}

impl MaybeVersionedFile {
    /// Get the URL of the file
    pub fn url(&self) -> &Url {
        match self {
            MaybeVersionedFile::Plain(f) => f.url(),
            MaybeVersionedFile::Versioned(f) => f.url(),
        }
    }

    /// Get the underlying ObjectStore
    pub fn store(&self) -> Arc<dyn ObjectStore> {
        match self {
            MaybeVersionedFile::Plain(f) => f.store(),
            MaybeVersionedFile::Versioned(f) => f.store(),
        }
    }

    /// Get the ObjectStore URL for DataFusion registration
    pub fn store_url(&self) -> ObjectStoreUrl {
        match self {
            MaybeVersionedFile::Plain(f) => f.store_url(),
            MaybeVersionedFile::Versioned(f) => f.store_url(),
        }
    }

    /// Get the path within the object store
    pub fn store_path(&self) -> &ObjectPath {
        match self {
            MaybeVersionedFile::Plain(f) => f.store_path(),
            MaybeVersionedFile::Versioned(f) => f.store_path(),
        }
    }

    /// Get the version of the file
    pub async fn version(&self) -> Result<String, BundlebaseError> {
        match self {
            MaybeVersionedFile::Plain(f) => f.version().await,
            MaybeVersionedFile::Versioned(f) => f.version().await,
        }
    }

    /// Get full ObjectMeta, validating version if needed
    pub async fn object_meta(&self) -> Result<Option<ObjectMeta>, BundlebaseError> {
        match self {
            MaybeVersionedFile::Plain(f) => f.object_meta().await,
            MaybeVersionedFile::Versioned(f) => f.object_meta().await,
        }
    }

    /// Get the underlying ObjectStoreFile reference.
    ///
    /// For versioned files, this returns the inner ObjectStoreFile.
    /// Note: When using a versioned file, ensure validation has been done first.
    pub fn as_object_store_file(&self) -> &ObjectStoreFile {
        match self {
            MaybeVersionedFile::Plain(f) => f,
            MaybeVersionedFile::Versioned(f) => f.inner(),
        }
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

    /// Create a reader for the given source.
    ///
    /// # Arguments
    /// * `source` - URL or path to the file
    /// * `bundle` - Bundle context (as trait object for flexibility)
    /// * `schema` - Optional schema (if already known)
    /// * `expected_version` - If provided, validates version on first data access
    pub async fn reader(
        &self,
        source: &str,
        bundle: &dyn BundleFacade,
        schema: Option<SchemaRef>,
        expected_version: Option<String>,
    ) -> Result<FileReader<C>, BundlebaseError> {
        let object_file =
            ObjectStoreFile::from_str(source, bundle.data_dir().as_ref(), bundle.config())?;

        let file = match expected_version {
            Some(v) => MaybeVersionedFile::Versioned(VersionedObjectStoreFile::new(object_file, v)),
            None => MaybeVersionedFile::Plain(object_file),
        };

        Ok(FileReader::new(
            file,
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
    file: MaybeVersionedFile,
    config: C,
    ctx: Arc<SessionContext>,
    schema: Option<SchemaRef>,
}

impl<C: FileFormatConfig> FileReader<C> {
    pub fn new(
        file: MaybeVersionedFile,
        config: C,
        ctx: Arc<SessionContext>,
        schema: Option<SchemaRef>,
    ) -> Self {
        Self {
            file,
            ctx,
            schema,
            config,
        }
    }
}

impl<C: FileFormatConfig> FileReader<C> {
    /// Get the file (MaybeVersionedFile)
    pub fn file(&self) -> &MaybeVersionedFile {
        &self.file
    }

    /// Get the URL of the file
    pub fn url(&self) -> &Url {
        self.file.url()
    }

    /// Get the object store
    pub fn object_store(&self) -> Arc<dyn ObjectStore> {
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
                // For RowIdOffsetDataSource, we need an ObjectStoreFile
                // Use as_object_store_file() which gives us access to the underlying file
                // Note: For versioned files, version validation happens on first object_meta() call
                // which occurs below if we fall through to the full scan path
                return Ok(Arc::new(RowIdOffsetDataSource::new(
                    self.file.as_object_store_file(),
                    self.schema.clone().expect("No schema set"),
                    ids.to_vec(),
                    projection.cloned(),
                    format,
                )));
            }
            // Format doesn't support line-oriented reading, fall back to full scan
            // This can happen with Parquet files
        }

        let metadata = self.file.object_meta().await.map_err(|e| {
            DataFusionError::Internal(format!("Failed to get object metadata: {}", e))
        })?.ok_or_else(|| {
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

impl<C: FileFormatConfig> std::fmt::Debug for FileReader<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("FileReader")
            .field("file", &self.file)
            .finish()
    }
}
