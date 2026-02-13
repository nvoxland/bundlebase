//! Throttled object store helpers for realistic benchmarks.
//!
//! Wraps a local filesystem store with `ThrottledStore` to simulate
//! cloud storage latencies (e.g., S3/GCS).

use bundlebase::io::{writable_dir_with_store, IOReadWriteDir};
use bundlebase::BundleConfig;
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::throttle::{ThrottleConfig, ThrottledStore};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use url::Url;

/// Returns a `ThrottleConfig` with S3-like latencies.
pub fn s3_like_config() -> ThrottleConfig {
    ThrottleConfig {
        wait_get_per_call: Duration::from_millis(75),
        wait_put_per_call: Duration::from_millis(75),
        wait_list_per_call: Duration::from_millis(100),
        wait_list_per_entry: Duration::from_millis(2),
        wait_list_with_delimiter_per_call: Duration::from_millis(100),
        wait_list_with_delimiter_per_entry: Duration::from_millis(2),
        wait_delete_per_call: Duration::from_millis(50),
        wait_get_per_byte: Duration::ZERO,
    }
}

/// Creates a throttled local-disk directory that simulates cloud storage latency.
///
/// Returns the writable directory and the `TempDir` handle (must be kept alive
/// for the directory to remain on disk).
pub fn throttled_local_dir(
    throttle_config: ThrottleConfig,
) -> (Arc<dyn IOReadWriteDir>, TempDir) {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let local_store = LocalFileSystem::new_with_prefix(tmp.path())
        .expect("failed to create local filesystem store");
    let throttled = ThrottledStore::new(local_store, throttle_config);
    let store: Arc<dyn object_store::ObjectStore> = Arc::new(throttled);

    let url = Url::from_directory_path(tmp.path()).expect("valid dir path");
    let path = ObjectPath::from("/");
    let config = Arc::new(BundleConfig::new(None).expect("config creation"));
    let dir = writable_dir_with_store(&url, store, &path, config)
        .expect("failed to create throttled dir");

    (dir, tmp)
}
