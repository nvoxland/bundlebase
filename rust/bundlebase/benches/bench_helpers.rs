//! Shared helpers for all benchmark files.
//!
//! Provides temp directory management, bundle creation, runtime construction,
//! and a `bench_main!` macro to eliminate boilerplate in each benchmark binary.

// This module is compiled into each benchmark binary; not all binaries
// use every constant or function, so dead-code warnings are expected.

#![allow(dead_code)]
use super::bench_data;
use super::bench_data::Format;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Runtime;
use bundlebase_command::BundleBuilderExt;

/// Root directory for benchmark temp files, under the system temp directory.
pub fn bench_tmp_dir() -> PathBuf {
    std::env::temp_dir().join("bundlebase")
}

/// Create a fresh subdirectory and return a `throttle://` URL string.
pub fn fresh_dir(prefix: &str) -> String {
    let dir = bench_tmp_dir().join(format!("{}_{}", prefix, rand::random::<u64>()));
    std::fs::create_dir_all(&dir).expect("failed to create bench tmp dir");
    format!("throttle://{}/", dir.display())
}


/// Clean up all benchmark temp files before a run.
pub fn clean_bench_tmp() {
    let tmp = bench_tmp_dir();
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).expect("failed to create bench tmp dir");
}

/// Create a multi-threaded tokio runtime for benchmarks.
pub fn create_runtime() -> Runtime {
    Runtime::new().expect("tokio runtime")
}

/// Create a bundle with synthetic data attached and committed.
pub async fn create_benchmark_bundle(
    rows: usize,
    format: &Format,
) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let data_url = bench_data::get_data_url(rows, format);
    let url = fresh_dir("bundle");
    let bundle = BundleBuilder::create(&url, None).await?;
    bundle.attach(&data_url, None).await?;
    bundle.commit("Setup").await?;
    Ok(bundle)
}

/// Standard benchmark main function: clean temp, register throttle, run benchmarks, clean temp.
macro_rules! bench_main {
    ($benches_fn:ident) => {
        fn main() {
            bundlebase_catalog::init();
            bench_helpers::clean_bench_tmp();
            throttled_store::register_throttle_scheme();
            $benches_fn();
            bench_helpers::clean_bench_tmp();
        }
    };
}

pub(crate) use bench_main;
