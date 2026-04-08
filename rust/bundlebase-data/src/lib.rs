#![deny(clippy::unwrap_used)]

pub mod attach_format;
pub mod column_stats_builder;
pub mod page_filter;
pub mod plugin;
pub mod reader_factory;
mod layout_cache;
pub mod physical_row_group_layout;
mod physical_row_group_data_source;
mod rowid_stream;

use bundlebase_common::config::ConfigProvider;
use bundlebase_common::BundlebaseError;
use bundlebase_io::IOReadWriteDir;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
pub use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;
pub use bundlebase_common::object_id::{BlockId, ObjectId, ObjectIdAlias};
pub use reader_factory::DataReaderFactory;
pub use bundlebase_common::row_id::{RowId, RowIdBatch, SendableRowIdBatchStream};
pub use layout_cache::GLOBAL_LAYOUT_CACHE;
pub use physical_row_group_layout::{ColumnStats, HistogramBucket, PageStats, PhysicalRowGroupLayout, StatValue, StringProfile};
pub use physical_row_group_data_source::{coalesce_page_ranges, LineOrientedFormat, PhysicalRowGroupDataSource};
pub use rowid_stream::RowIdStreamAdapter;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use url::Url;
pub use bundlebase_common::versioned_blockid::VersionedBlockId;
pub use plugin::ReaderPlugin;

pub trait DataContext: Send + Sync {
    fn config_provider(&self) -> Arc<dyn ConfigProvider>;
    fn data_context_dir(&self) -> Arc<dyn IOReadWriteDir>;
    fn session_context(&self) -> Arc<SessionContext>;
}

#[async_trait]
pub trait DataReader: Sync + Send + Debug {
    fn url(&self) -> &Url;

    fn block_id(&self) -> BlockId;

    /// The attach format this reader handles.
    fn format(&self) -> crate::attach_format::AttachFormat;

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError>;

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError>;

    async fn read_version(&self) -> Result<String, BundlebaseError>;

    /// Return pre-computed per-column statistics captured at attach time, if available.
    ///
    /// The returned `Vec` is positional: index N corresponds to column N in the reader's schema.
    /// The default implementation returns an empty Vec (no pre-computed stats).
    /// CSV and JSONL readers override this to load stats from the layout file.
    async fn column_stats(&self) -> Result<Vec<crate::physical_row_group_layout::ColumnStats>, BundlebaseError> {
        Ok(vec![])
    }

    /// Return a filtered data source that reads only pages whose per-page min/max stats
    /// overlap with the given filter predicates.
    ///
    /// Returns `Ok(None)` when page-level filtering is not supported or no pruning is possible.
    /// When `Some` is returned, the data source is partial and must NOT be placed in the block
    /// cache (which stores complete blocks only).
    async fn data_source_filtered_pages(
        &self,
        _projection: Option<&Vec<usize>>,
        _filters: &[datafusion::logical_expr::Expr],
        _limit: Option<usize>,
    ) -> Result<Option<Arc<dyn DataSource>>, BundlebaseError> {
        Ok(None)
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError>;

    async fn build_layout(
        &self,
        _data_dir: &dyn IOReadWriteDir,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadFile>>, BundlebaseError> {
        Ok(None)
    }

    /// Read specific rows by their RowIds
    /// Returns a stream of RecordBatches containing only the requested rows
    async fn read_rows_by_ids(
        &self,
        _row_ids: &[RowId],
        _projection: Option<&Vec<usize>>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Err("read_rows_by_ids not implemented for this adapter".into())
    }

    /// Return format-specific options detected during schema inference.
    ///
    /// These options are stored in the attach operation and passed back when
    /// creating readers for subsequent reads. For example, the CSV reader
    /// may detect that `newlines_in_values` is required.
    fn read_options(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    /// Stream data with RowIds for index building.
    /// Each batch is paired with sequential logical RowIds.
    ///
    /// # Arguments
    /// * `block_ref` - The compact ObjectIdAlias to embed in each RowId for this block
    /// * `ctx` - DataFusion session context
    /// * `projection` - Optional column projection
    async fn extract_rowids_stream(
        &self,
        block_ref: ObjectIdAlias,
        ctx: Arc<SessionContext>,
        projection: Option<&Vec<usize>>,
    ) -> Result<SendableRowIdBatchStream, BundlebaseError> {
        let data_source = self
            .data_source(projection, &[], None, None)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        let record_batch_stream = data_source
            .open(0, ctx.task_ctx())
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        Ok(Box::pin(RowIdStreamAdapter::new(
            record_batch_stream,
            block_ref,
        )))
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    use super::DataContext;
    use bundlebase_common::config::ConfigProvider;
    use bundlebase_common::{BundlebaseError, ConfigKey, Scope};
    use bundlebase_io::IOReadWriteDir;
    use bundlebase_io::plugin::object_store::ObjectStoreFile;
    use bundlebase_io::file::IOReadWriteFile;
    use datafusion::prelude::SessionContext;
    use std::sync::Arc;
    use url::Url;

    pub struct TestDataContext {
        pub config: Arc<dyn ConfigProvider>,
        pub dir: Arc<dyn IOReadWriteDir>,
        pub ctx: Arc<SessionContext>,
    }

    impl DataContext for TestDataContext {
        fn config_provider(&self) -> Arc<dyn ConfigProvider> { self.config.clone() }
        fn data_context_dir(&self) -> Arc<dyn IOReadWriteDir> { self.dir.clone() }
        fn session_context(&self) -> Arc<SessionContext> { self.ctx.clone() }
    }

    struct EmptyConfig;
    impl ConfigProvider for EmptyConfig {
        fn get(&self, _: &Scope, _: &ConfigKey) -> Result<Option<String>, BundlebaseError> { Ok(None) }
    }

    pub fn test_config() -> Arc<dyn ConfigProvider> {
        Arc::new(EmptyConfig)
    }

    pub fn test_context() -> TestDataContext {
        let config: Arc<dyn ConfigProvider> = test_config();
        let url = Url::parse("memory:///test_data_ctx").expect("valid url");
        let store = bundlebase_io::get_memory_store();
        let dir = bundlebase_io::writable_dir_with_store(
            &url,
            store.clone(),
            &object_store::path::Path::from(url.path()),
            config.clone(),
        ).expect("valid dir");
        let ctx = SessionContext::new();
        let memory_url = datafusion::datasource::object_store::ObjectStoreUrl::parse("memory://").expect("valid url");
        ctx.register_object_store(memory_url.as_ref(), store);
        TestDataContext {
            config,
            dir,
            ctx: Arc::new(ctx),
        }
    }

    static TEST_DATAFILE_RESPONSES: std::sync::OnceLock<std::collections::HashMap<String, String>> = std::sync::OnceLock::new();

    pub fn test_datafile(name: &str) -> &'static str {
        let responses = TEST_DATAFILE_RESPONSES.get_or_init(|| {
            std::thread::spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
                let mut map = std::collections::HashMap::new();
                let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("test_data"))
                    .expect("test_data dir");
                for datafile in data_dir.read_dir().expect("read test_data") {
                    let os_path = datafile.expect("entry").path();
                    let filename = os_path.file_name().expect("filename").to_str().expect("str").to_string();
                    let bytes = std::fs::read(&os_path).expect("read file");

                    let url = Url::parse(&format!("memory:///test_data/{}", filename)).expect("valid url");
                    let file = ObjectStoreFile::from_url(&url, test_config()).expect("file");

                    rt.block_on(file.write(bytes.into())).expect("write");

                    map.insert(filename, url.to_string());
                }
                map
            })
            .join()
            .expect("thread panicked while initializing test datafiles")
        });

        responses
            .get(name)
            .unwrap_or_else(|| panic!("test_datafile: no datafile `{}`", name))
            .as_str()
    }
}
