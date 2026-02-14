//! Streaming and memory benchmarks
//!
//! Critical benchmarks for verifying constant memory usage during streaming.
//! The key constraint is ~50MB constant memory regardless of dataset size.

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::{Format, ALL_FORMATS};
use bench_helpers::{create_benchmark_bundle, create_runtime};
use bundlebase::bundle::BundleFacade;
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K, SCALE_1M};
use futures::StreamExt;

fn bench_stream_rows(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("stream_rows");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K, SCALE_1M] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format.name(), rows),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_benchmark_bundle(rows, &format))
                        .expect("bundle creation");

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
        }
    }
    group.finish();
}

fn bench_stream_with_filter(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("stream_with_filter");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K, SCALE_1M] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format.name(), rows),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_benchmark_bundle(rows, &format))
                        .expect("bundle creation");

                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            bundle
                                .filter(
                                    "SELECT * FROM bundle WHERE filter_value < 50",
                                    vec![],
                                )
                                .await
                                .expect("filter failed");
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
        }
    }
    group.finish();
}

fn bench_stream_with_aggregation(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("stream_with_aggregation");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K, SCALE_1M] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format.name(), rows),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_benchmark_bundle(rows, &format))
                        .expect("bundle creation");

                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT category, SUM(amount) as total FROM bundle GROUP BY category",
                                    vec![],
                                )
                                .await
                                .expect("query failed");

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
    }
    group.finish();
}

fn bench_stream_projection(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("stream_projection");

    for format in ALL_FORMATS {
        for rows in [SCALE_100K, SCALE_1M] {
            // Stream all columns
            group.bench_with_input(
                BenchmarkId::new(format.name(), format!("{}_all_cols", rows)),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_benchmark_bundle(rows, &format))
                        .expect("bundle creation");

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
                BenchmarkId::new(format.name(), format!("{}_2_cols", rows)),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_benchmark_bundle(rows, &format))
                        .expect("bundle creation");

                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT id, amount FROM bundle", vec![])
                                .await
                                .expect("query failed");

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
    }
    group.finish();
}

/// Stream 1M rows benchmark - processes 1M rows through streaming
/// This benchmark verifies streaming throughput at scale
fn bench_stream_1m_rows(c: &mut Criterion) {
    let rt = create_runtime();

    // Only test with parquet — streaming throughput is format-independent at this point
    c.bench_function("stream_1m_rows", |b| {
        let bundle = rt
            .block_on(create_benchmark_bundle(SCALE_1M, &Format::Parquet))
            .expect("bundle creation");

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
    bench_stream_1m_rows,
);

bench_helpers::bench_main!(benches);
