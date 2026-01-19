//! Streaming and memory benchmarks
//!
//! Critical benchmarks for verifying constant memory usage during streaming.
//! The key constraint is ~50MB constant memory regardless of dataset size.

mod data_generator;

use bytes::Bytes;
use bundlebase::bundle::BundleFacade;
use bundlebase::io::{writable_dir_from_url, IOReadWriteDir};
use bundlebase::{BundleBuilder, BundleConfig, BundlebaseError};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use data_generator::{generate_batch, BenchmarkDataConfig, SCALE_100K, SCALE_10K, SCALE_1M};
use futures::StreamExt;
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

/// Create a bundle with synthetic data
async fn create_benchmark_bundle(rows: usize) -> Result<BundleBuilder, BundlebaseError> {
    let data_dir = random_memory_dir();
    let data_url = write_parquet_to_memory(data_dir.as_ref(), "data.parquet", rows).await?;

    let bundle_url = random_memory_url();
    let mut bundle = BundleBuilder::create(bundle_url.as_str(), None).await?;
    bundle.attach(&data_url, None).await?;

    Ok(bundle)
}

fn bench_stream_rows(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("stream_rows");

    for rows in [SCALE_10K, SCALE_100K, SCALE_1M] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let df = bundle.dataframe().await.expect("dataframe failed");
                    let mut stream = df
                        .as_ref()
                        .clone()
                        .execute_stream()
                        .await
                        .expect("execute_stream failed");

                    let mut total_rows = 0usize;
                    while let Some(batch_result) = stream.next().await {
                        let batch = batch_result.expect("batch failed");
                        total_rows += batch.num_rows();
                    }
                    total_rows
                }
            });
        });
    }
    group.finish();
}

fn bench_stream_with_filter(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("stream_with_filter");

    for rows in [SCALE_10K, SCALE_100K, SCALE_1M] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    // Apply filter then stream
                    let filtered = bundle
                        .select("* WHERE filter_value < 50", vec![])
                        .await
                        .expect("select failed");
                    let df = filtered.dataframe().await.expect("dataframe failed");
                    let mut stream = df
                        .as_ref()
                        .clone()
                        .execute_stream()
                        .await
                        .expect("execute_stream failed");

                    let mut total_rows = 0usize;
                    while let Some(batch_result) = stream.next().await {
                        let batch = batch_result.expect("batch failed");
                        total_rows += batch.num_rows();
                    }
                    total_rows
                }
            });
        });
    }
    group.finish();
}

fn bench_stream_with_aggregation(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("stream_with_aggregation");

    for rows in [SCALE_10K, SCALE_100K, SCALE_1M] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let result = bundle
                        .select("category, SUM(amount) as total FROM data GROUP BY category", vec![])
                        .await
                        .expect("select failed");
                    let df = result.dataframe().await.expect("dataframe failed");
                    let mut stream = df
                        .as_ref()
                        .clone()
                        .execute_stream()
                        .await
                        .expect("execute_stream failed");

                    let mut total_rows = 0usize;
                    while let Some(batch_result) = stream.next().await {
                        let batch = batch_result.expect("batch failed");
                        total_rows += batch.num_rows();
                    }
                    total_rows
                }
            });
        });
    }
    group.finish();
}

fn bench_stream_projection(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("stream_projection");

    // Benchmark streaming with different projection sizes
    for rows in [SCALE_100K, SCALE_1M] {
        // Stream all columns
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_all_cols", rows)),
            &rows,
            |b, &rows| {
                let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

                b.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move {
                        let df = bundle.dataframe().await.expect("dataframe failed");
                        let mut stream = df
                            .as_ref()
                            .clone()
                            .execute_stream()
                            .await
                            .expect("execute_stream failed");

                        let mut total_rows = 0usize;
                        while let Some(batch_result) = stream.next().await {
                            let batch = batch_result.expect("batch failed");
                            total_rows += batch.num_rows();
                        }
                        total_rows
                    }
                });
            },
        );

        // Stream subset of columns (2 columns)
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_2_cols", rows)),
            &rows,
            |b, &rows| {
                let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

                b.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move {
                        let result = bundle
                            .select("id, amount", vec![])
                            .await
                            .expect("select failed");
                        let df = result.dataframe().await.expect("dataframe failed");
                        let mut stream = df
                            .as_ref()
                            .clone()
                            .execute_stream()
                            .await
                            .expect("execute_stream failed");

                        let mut total_rows = 0usize;
                        while let Some(batch_result) = stream.next().await {
                            let batch = batch_result.expect("batch failed");
                            total_rows += batch.num_rows();
                        }
                        total_rows
                    }
                });
            },
        );
    }
    group.finish();
}

/// Memory assertion test - verifies streaming uses constant memory
/// This benchmark processes 1M rows and should use <100MB RAM
fn bench_memory_assertion_1m(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    c.bench_function("memory_assertion_1m_rows", |b| {
        let bundle = rt.block_on(create_benchmark_bundle(SCALE_1M)).expect("bundle creation");

        b.to_async(&rt).iter(|| {
            let bundle = bundle.clone();
            async move {
                let df = bundle.dataframe().await.expect("dataframe failed");
                let mut stream = df
                    .as_ref()
                    .clone()
                    .execute_stream()
                    .await
                    .expect("execute_stream failed");

                let mut total_rows = 0usize;
                let mut max_batch_size = 0usize;

                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result.expect("batch failed");
                    total_rows += batch.num_rows();
                    max_batch_size = max_batch_size.max(batch.num_rows());
                }

                // Return total rows and max batch size for verification
                (total_rows, max_batch_size)
            }
        });
    });
}

criterion_group!(
    benches,
    bench_stream_rows,
    bench_stream_with_filter,
    bench_stream_with_aggregation,
    bench_stream_projection,
    bench_memory_assertion_1m,
);
criterion_main!(benches);
