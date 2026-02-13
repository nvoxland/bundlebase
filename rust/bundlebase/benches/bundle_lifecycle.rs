//! Bundle lifecycle benchmarks
//!
//! Benchmarks for create, open, attach, and commit operations.
//! All data is written to disk under $TMPDIR/bundlebase/ (cleaned per run).
//!
//! BundleBuilder also has an Arc reference cycle (BundleBuilder → Bundle →
//! SessionContext → SchemaProviders → BundleBuilder). We break this cycle
//! after each iteration to prevent memory leaks during long benchmark runs.

mod bench_data;
mod data_generator;
mod throttled_store;

use bench_data::Format;
use bundlebase::{BundleBuilder, BundleFacade};
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use datafusion::catalog::MemorySchemaProvider;
use data_generator::{SCALE_10K, SCALE_1K};
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

/// Root directory for benchmark temp files, under the system temp directory.
fn bench_tmp_dir() -> PathBuf {
    std::env::temp_dir().join("bundlebase")
}

/// Create a fresh subdirectory under $TMPDIR/bundlebase/ with a random name.
/// Returns the directory path and a file:// URL pointing to it.
fn fresh_disk_dir(prefix: &str) -> (PathBuf, Url) {
    let dir = bench_tmp_dir().join(format!("{}_{}", prefix, rand::random::<u64>()));
    std::fs::create_dir_all(&dir).expect("failed to create bench tmp dir");
    let url = Url::from_directory_path(&dir).expect("valid dir path");
    (dir, url)
}

/// Clean up all benchmark temp files before a run.
fn clean_bench_tmp() {
    let tmp = bench_tmp_dir();
    if tmp.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    std::fs::create_dir_all(&tmp).expect("failed to create bench tmp dir");
}

/// Break the Arc reference cycle in a BundleBuilder so it can be freed.
///
/// BundleBuilder → Bundle → SessionContext → SchemaProviders → BundleBuilder
/// forms a cycle that prevents Drop. Replacing the schema providers with empty
/// MemorySchemaProviders drops the `Arc<dyn BundleFacade>` references, breaking
/// the cycle.
fn break_arc_cycle(bundle: &BundleBuilder) {
    let ctx = bundle.ctx();
    if let Some(catalog) = ctx.catalog("bundlebase") {
        let empty = Arc::new(MemorySchemaProvider::new());
        let _ = catalog.register_schema("blocks", empty.clone());
        let _ = catalog.register_schema("packs", empty.clone());
        let _ = catalog.register_schema("default", empty.clone());
        let _ = catalog.register_schema("bundle_info", empty);
    }
}

/// Clean up after a benchmark iteration.
///
/// Breaks the Arc reference cycle to allow the BundleBuilder to be freed.
fn cleanup_after_iter(bundle: &BundleBuilder) {
    break_arc_cycle(bundle);
}

fn bench_create_bundle(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    c.bench_function("create_empty_bundle", |b| {
        b.to_async(&rt).iter(|| async {
            let (_path, url) = fresh_disk_dir("bundle");
            let bundle = BundleBuilder::create(url.as_str(), None)
                .await
                .expect("bundle creation failed");
            bundle.commit("Created bundle").await.expect("Commit failed");
            cleanup_after_iter(&bundle);
        });
    });
}

fn bench_attach_data(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("attach_data");
    group.sample_size(10);

    for rows in [SCALE_1K, SCALE_10K] {
        let data_url = bench_data::get_data_url(rows, &Format::Parquet);

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &_rows| {
            let data_url = data_url.clone();
            b.to_async(&rt).iter(|| {
                let data_url = data_url.clone();
                async move {
                    let (_path, url) = fresh_disk_dir("bundle");
                    let bundle = BundleBuilder::create(url.as_str(), None)
                        .await
                        .expect("bundle creation failed");
                    bundle
                        .attach(&data_url, None)
                        .await
                        .expect("attach failed");
                    bundle.commit("Attached file").await.expect("commit failed");
                    cleanup_after_iter(&bundle);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_create_bundle,
    bench_attach_data,
);

fn main() {
    clean_bench_tmp();
    benches();
    clean_bench_tmp();
}
