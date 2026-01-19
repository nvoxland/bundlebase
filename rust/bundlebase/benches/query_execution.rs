//! Query execution benchmarks
//!
//! Benchmarks for filter, select, aggregation, and join operations.

mod data_generator;

use bytes::Bytes;
use bundlebase::bundle::BundleFacade;
use bundlebase::io::{writable_dir_from_url, IOReadWriteDir};
use bundlebase::{BundleBuilder, BundleConfig, BundlebaseError, JoinTypeOption};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use data_generator::{generate_batch, generate_lookup_batch, BenchmarkDataConfig, SCALE_100K, SCALE_10K, SCALE_1K};
use datafusion::common::ScalarValue;
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

/// Write a lookup table to a parquet file in memory
async fn write_lookup_to_memory(
    dir: &dyn IOReadWriteDir,
    name: &str,
    rows: usize,
) -> Result<String, BundlebaseError> {
    let batch = generate_lookup_batch(rows);

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

/// Create a bundle with synthetic data for query benchmarks
async fn create_benchmark_bundle(rows: usize) -> Result<BundleBuilder, BundlebaseError> {
    let data_dir = random_memory_dir();
    let data_url = write_parquet_to_memory(data_dir.as_ref(), "data.parquet", rows).await?;

    let bundle_url = random_memory_url();
    let mut bundle = BundleBuilder::create(bundle_url.as_str(), None).await?;
    bundle.attach(&data_url, None).await?;

    Ok(bundle)
}

fn bench_filter_selective(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("filter_selective");

    // 1% selectivity filter: filter_value < 1 (filter_value is 0-99)
    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let filtered = bundle
                        .select("* WHERE filter_value < 1", vec![])
                        .await
                        .expect("select failed");
                    let df = filtered.dataframe().await.expect("dataframe failed");
                    // Note: Using collect() for benchmarking only - production should use streaming
                    let _result = df.as_ref().clone().collect().await.expect("collect failed");
                }
            });
        });
    }
    group.finish();
}

fn bench_filter_broad(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("filter_broad");

    // 50% selectivity filter: filter_value < 50
    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let filtered = bundle
                        .select("* WHERE filter_value < 50", vec![])
                        .await
                        .expect("select failed");
                    let df = filtered.dataframe().await.expect("dataframe failed");
                    let _result = df.as_ref().clone().collect().await.expect("collect failed");
                }
            });
        });
    }
    group.finish();
}

fn bench_aggregation_sum(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("aggregation_sum");

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                    let _result = df.as_ref().clone().collect().await.expect("collect failed");
                }
            });
        });
    }
    group.finish();
}

fn bench_filter_parameterized(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("filter_parameterized");

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut filtered = bundle.clone();
                    filtered
                        .filter("filter_value < $1", vec![ScalarValue::Int64(Some(50))])
                        .await
                        .expect("filter failed");
                    let df = filtered.dataframe().await.expect("dataframe failed");
                    let _result = df.as_ref().clone().collect().await.expect("collect failed");
                }
            });
        });
    }
    group.finish();
}

fn bench_join_small_large(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("join_small_large");

    // Join 1K lookup table with 10K-100K main table
    for main_rows in [SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(main_rows as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("1k_with_{}", main_rows)),
            &main_rows,
            |b, &main_rows| {
                // Setup: create main bundle and lookup data
                let (bundle, lookup_url) = rt.block_on(async {
                    let data_dir = random_memory_dir();
                    let main_url =
                        write_parquet_to_memory(data_dir.as_ref(), "main.parquet", main_rows)
                            .await
                            .expect("main parquet failed");
                    let lookup_url =
                        write_lookup_to_memory(data_dir.as_ref(), "lookup.parquet", SCALE_1K)
                            .await
                            .expect("lookup parquet failed");

                    let bundle_url = random_memory_url();
                    let mut bundle = BundleBuilder::create(bundle_url.as_str(), None)
                        .await
                        .expect("bundle creation failed");
                    bundle.attach(&main_url, None).await.expect("attach failed");

                    (bundle, lookup_url)
                });

                b.to_async(&rt).iter(|| {
                    let mut bundle = bundle.clone();
                    let lookup_url = lookup_url.clone();
                    async move {
                        bundle
                            .join(
                                "lookup",
                                "id = lookup_id",
                                Some(&lookup_url),
                                JoinTypeOption::Left,
                            )
                            .await
                            .expect("join failed");
                        let df = bundle.dataframe().await.expect("dataframe failed");
                        let _result = df.as_ref().clone().collect().await.expect("collect failed");
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_projection(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("projection");

    // Select subset of columns
    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            let bundle = rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation");

            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let result = bundle
                        .select("id, category, amount", vec![])
                        .await
                        .expect("select failed");
                    let df = result.dataframe().await.expect("dataframe failed");
                    let _result = df.as_ref().clone().collect().await.expect("collect failed");
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_filter_selective,
    bench_filter_broad,
    bench_aggregation_sum,
    bench_filter_parameterized,
    bench_join_small_large,
    bench_projection,
);
criterion_main!(benches);
