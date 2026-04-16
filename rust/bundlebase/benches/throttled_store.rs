//! Throttled object store helpers for realistic benchmarks.
//!
//! Registers a `throttle://` URL scheme so that a throttled `LocalFileSystem`
//! is created transparently. This survives the commit-reopen cycle because
//! the URL is preserved and the factory is global.

use bundlebase_io::{io_registry, register_object_store_scheme};
use futures::stream::BoxStream;
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use std::sync::Arc;
use std::time::Duration;

/// S3-like latency configuration for throttled stores.
#[derive(Debug, Clone)]
pub struct ThrottleConfig {
    pub wait_get_per_call: Duration,
    pub wait_put_per_call: Duration,
    pub wait_list_per_call: Duration,
    pub wait_delete_per_call: Duration,
}

/// Returns a `ThrottleConfig` with S3-like latencies.
pub fn s3_like_config() -> ThrottleConfig {
    ThrottleConfig {
        wait_get_per_call: Duration::from_millis(75),
        wait_put_per_call: Duration::from_millis(75),
        wait_list_per_call: Duration::from_millis(100),
        wait_delete_per_call: Duration::from_millis(50),
    }
}

/// A `LocalFileSystem` wrapper that adds configurable per-call delays to
/// simulate cloud storage latency.
#[derive(Debug)]
struct ThrottledLocalStore {
    inner: Arc<LocalFileSystem>,
    config: ThrottleConfig,
}

impl std::fmt::Display for ThrottledLocalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThrottledLocalStore({})", self.inner)
    }
}

#[async_trait::async_trait]
impl ObjectStore for ThrottledLocalStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        tokio::time::sleep(self.config.wait_get_per_call).await;
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<ObjectPath>>,
    ) -> BoxStream<'static, object_store::Result<ObjectPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        tokio::time::sleep(self.config.wait_list_per_call).await;
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.copy_opts(from, to, options).await
    }
}

/// Register the `throttle://` scheme with S3-like latencies.
///
/// Registers at two levels:
/// 1. ObjectStore custom scheme — so `parse_url` creates a throttled store
/// 2. IORegistry dynamic factory — so the `io_registry()` dispatches `throttle://` URLs
///    to the ObjectStore factory which then creates the throttled store
pub fn register_throttle_scheme() {
    let throttle_config = s3_like_config();

    // Register the throttled ObjectStore factory for the "throttle" scheme
    register_object_store_scheme("throttle", move |url, _config| {
        let local_store = Arc::new(LocalFileSystem::new());
        let store = ThrottledLocalStore {
            inner: local_store,
            config: throttle_config.clone(),
        };
        Ok((Arc::new(store), ObjectPath::from(url.path())))
    });

    // Register with the IORegistry so that writable_dir_from_url etc. can resolve "throttle://"
    // The ObjectStoreIOFactory handles it via parse_url → CUSTOM_SCHEMES
    let object_store_factory = io_registry()
        .get_factory("file")
        .expect("file factory must be registered");
    io_registry().register_dynamic(Arc::new(ThrottleIOFactory {
        delegate: object_store_factory,
    }));
}

/// IOFactory that delegates to the ObjectStore factory for "throttle://" URLs.
/// The underlying ObjectStore parse_url will find the throttled store via CUSTOM_SCHEMES.
struct ThrottleIOFactory {
    delegate: Arc<dyn bundlebase_io::IOFactory>,
}

#[async_trait::async_trait]
impl bundlebase_io::IOFactory for ThrottleIOFactory {
    fn schemes(&self) -> &[&str] {
        &["throttle"]
    }

    async fn create_reader(
        &self,
        url: &url::Url,
        config: Arc<dyn bundlebase_io::ConfigProvider>,
    ) -> Result<Box<dyn bundlebase_io::IOReadFile>, bundlebase_common::BundlebaseError> {
        self.delegate.create_reader(url, config).await
    }

    async fn create_lister(
        &self,
        url: &url::Url,
        config: Arc<dyn bundlebase_io::ConfigProvider>,
    ) -> Result<Box<dyn bundlebase_io::IOReadDir>, bundlebase_common::BundlebaseError> {
        self.delegate.create_lister(url, config).await
    }

    async fn create_writable_lister(
        &self,
        url: &url::Url,
        config: Arc<dyn bundlebase_io::ConfigProvider>,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadWriteDir>>, bundlebase_common::BundlebaseError>
    {
        self.delegate.create_writable_lister(url, config).await
    }

    async fn create_writer(
        &self,
        url: &url::Url,
        config: Arc<dyn bundlebase_io::ConfigProvider>,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadWriteFile>>, bundlebase_common::BundlebaseError>
    {
        self.delegate.create_writer(url, config).await
    }
}
