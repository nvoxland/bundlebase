//! Bundle lifecycle benchmarks
//!
//! Benchmarks for create, open, attach, and commit operations.

mod data_generator;

use bytes::Bytes;
use bundlebase::io::{writable_dir_from_url, IOReadWriteDir};
use bundlebase::{Bundle, BundleBuilder, BundleConfig, BundlebaseError};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use data_generator::{generate_batch, BenchmarkDataConfig, SCALE_100K, SCALE_10K, SCALE_1K};
use parquet::arrow::ArrowWriter;
use std::sync::Arc;
use tokio::runtime::Runtime;
use url::Url;

fn random_memory_url() -> Url {
    Url::parse(&format!("memory://bench/{}", rand::random::<u64>())).expect("valid url")
}

fn random_memory_dir() -> Arc<dyn IOReadWriteDir> {
    writable_dir_from_url(&random_memory_url(), BundleConfig::default().into())
        .expect("failed to create memory dir")
}

/// Write a RecordBatch to a parquet file in memory
async fn write_parquet_to_memory(
    dir: &dyn IOReadWriteDir,
    name: &str,
    rows: usize,
) -> Result<String, BundlebaseError> {
    let config = BenchmarkDataConfig::with_rows(rows);
    let batch = generate_batch(&config);

    let mut buffer = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), None)
            .map_err(|e| BundlebaseError::from(e.to_string()))?;
        writer
            .write(&batch)
            .map_err(|e| BundlebaseError::from(e.to_string()))?;
        writer
            .close()
            .map_err(|e| BundlebaseError::from(e.to_string()))?;
    }

    let file = dir.writable_file(name)?;
    file.write(Bytes::from(buffer)).await?;

    Ok(file.url().to_string())
}

fn bench_create_empty(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    c.bench_function("create_empty_bundle", |b| {
        b.to_async(&rt).iter(|| async {
            let url = random_memory_url();
            BundleBuilder::create(url.as_str(), None)
                .await
                .expect("bundle creation failed")
        });
    });
}

fn bench_create_with_data(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("create_with_data");

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            b.to_async(&rt).iter(|| async move {
                // Create data file
                let data_dir = random_memory_dir();
                let data_url = write_parquet_to_memory(data_dir.as_ref(), "data.parquet", rows)
                    .await
                    .expect("parquet write failed");

                // Create bundle and attach
                let bundle_dir = random_memory_url();
                let mut bundle = BundleBuilder::create(bundle_dir.as_str(), None)
                    .await
                    .expect("bundle creation failed");

                bundle
                    .attach(&data_url, None)
                    .await
                    .expect("attach failed");

                bundle
            });
        });
    }
    group.finish();
}

fn bench_open_bundle(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    // Setup: Create and commit a bundle with data
    let (bundle_url, _data_url) = rt.block_on(async {
        let data_dir = random_memory_dir();
        let data_url = write_parquet_to_memory(data_dir.as_ref(), "data.parquet", SCALE_10K)
            .await
            .expect("parquet write failed");

        let bundle_url = random_memory_url();
        let mut bundle = BundleBuilder::create(bundle_url.as_str(), None)
            .await
            .expect("bundle creation failed");

        bundle
            .attach(&data_url, None)
            .await
            .expect("attach failed");
        bundle.commit("Initial commit").await.expect("commit failed");

        (bundle_url, data_url)
    });

    c.bench_function("open_bundle", |b| {
        b.to_async(&rt).iter(|| async {
            Bundle::open(bundle_url.as_str(), None)
                .await
                .expect("open failed")
        });
    });
}

fn bench_commit(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("commit_bundle");

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            b.to_async(&rt).iter_batched(
                || {
                    // Setup: create bundle with data but don't commit
                    rt.block_on(async {
                        let data_dir = random_memory_dir();
                        let data_url =
                            write_parquet_to_memory(data_dir.as_ref(), "data.parquet", rows)
                                .await
                                .expect("parquet write failed");

                        let bundle_url = random_memory_url();
                        let mut bundle = BundleBuilder::create(bundle_url.as_str(), None)
                            .await
                            .expect("bundle creation failed");

                        bundle
                            .attach(&data_url, None)
                            .await
                            .expect("attach failed");

                        bundle
                    })
                },
                |mut bundle| async move {
                    bundle.commit("Benchmark commit").await.expect("commit failed");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_attach_multiple(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("attach_multiple");

    for num_files in [1, 5, 10] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_files", num_files)),
            &num_files,
            |b, &num_files| {
                b.to_async(&rt).iter_batched(
                    || {
                        // Setup: create data files
                        rt.block_on(async {
                            let data_dir = random_memory_dir();
                            let mut urls = Vec::new();
                            for i in 0..num_files {
                                let url = write_parquet_to_memory(
                                    data_dir.as_ref(),
                                    &format!("data_{}.parquet", i),
                                    SCALE_1K,
                                )
                                .await
                                .expect("parquet write failed");
                                urls.push(url);
                            }
                            urls
                        })
                    },
                    |urls| async move {
                        let bundle_url = random_memory_url();
                        let mut bundle = BundleBuilder::create(bundle_url.as_str(), None)
                            .await
                            .expect("bundle creation failed");

                        for url in urls {
                            bundle.attach(&url, None).await.expect("attach failed");
                        }
                        bundle
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_create_empty,
    bench_create_with_data,
    bench_open_bundle,
    bench_commit,
    bench_attach_multiple,
);
criterion_main!(benches);
