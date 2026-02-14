//! Throttled object store helpers for realistic benchmarks.
//!
//! Registers a `throttle://` URL scheme so that a throttled `LocalFileSystem`
//! is created transparently by `parse_url()`. This survives the commit-reopen
//! cycle because the URL is preserved and the factory is global.
//!
//! We implement our own throttling wrapper instead of using `ThrottledStore`
//! because `ThrottledStore` panics on `GetResultPayload::File` (returned by
//! `LocalFileSystem`) and doesn't implement `rename`.

use bundlebase::io::register_object_store_scheme;
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{
    GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
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
/// simulate cloud storage latency. Unlike `object_store::ThrottledStore`,
/// this handles `GetResultPayload::File` and supports `rename`.
#[derive(Debug)]
struct ThrottledLocalStore {
    inner: LocalFileSystem,
    config: ThrottleConfig,
}

impl std::fmt::Display for ThrottledLocalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThrottledLocalStore({})", self.inner)
    }
}

#[async_trait::async_trait]
impl ObjectStore for ThrottledLocalStore {
    async fn put(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
    ) -> object_store::Result<PutResult> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.put(location, payload).await
    }

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

    async fn get(&self, location: &ObjectPath) -> object_store::Result<GetResult> {
        tokio::time::sleep(self.config.wait_get_per_call).await;
        self.inner.get(location).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        tokio::time::sleep(self.config.wait_get_per_call).await;
        self.inner.get_opts(location, options).await
    }

    async fn head(&self, location: &ObjectPath) -> object_store::Result<ObjectMeta> {
        tokio::time::sleep(self.config.wait_get_per_call).await;
        self.inner.head(location).await
    }

    async fn delete(&self, location: &ObjectPath) -> object_store::Result<()> {
        tokio::time::sleep(self.config.wait_delete_per_call).await;
        self.inner.delete(location).await
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> futures::stream::BoxStream<'static, object_store::Result<ObjectMeta>> {
        // Note: no async delay on list since it returns a stream synchronously.
        // The latency cost is amortized across iteration.
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        tokio::time::sleep(self.config.wait_list_per_call).await;
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy(&self, from: &ObjectPath, to: &ObjectPath) -> object_store::Result<()> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.copy(from, to).await
    }

    async fn copy_if_not_exists(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
    ) -> object_store::Result<()> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.copy_if_not_exists(from, to).await
    }

    async fn rename(&self, from: &ObjectPath, to: &ObjectPath) -> object_store::Result<()> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.rename(from, to).await
    }

    async fn rename_if_not_exists(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
    ) -> object_store::Result<()> {
        tokio::time::sleep(self.config.wait_put_per_call).await;
        self.inner.rename_if_not_exists(from, to).await
    }
}

/// Register the `throttle://` scheme with S3-like latencies.
///
/// After calling this, any URL like `throttle:///path/to/dir` will create a
/// throttled `LocalFileSystem` backed by the filesystem path.
/// Safe to call multiple times (re-registers with the same factory).
pub fn register_throttle_scheme() {
    let throttle_config = s3_like_config();

    register_object_store_scheme("throttle", move |url, _config| {
        let local_store = LocalFileSystem::new();
        let store = ThrottledLocalStore {
            inner: local_store,
            config: throttle_config.clone(),
        };
        Ok((Arc::new(store), ObjectPath::from(url.path())))
    });
}
