//! Query execution benchmarks
//!
//! Benchmarks for filter, select, aggregation, and join operations.

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::ALL_FORMATS;
use bench_helpers::{create_benchmark_bundle, create_runtime, fresh_dir};
use bundlebase::bundle::BundleFacade;
use bundlebase::{BundleBuilder, JoinTypeOption};
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K, SCALE_1K};
use datafusion::common::ScalarValue;
use futures::StreamExt;
use bundlebase_command::BundleBuilderExt;

fn bench_filter_selective(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("filter_selective");

    for format in ALL_FORMATS {
        // 1% selectivity filter: filter_value < 1 (filter_value is 0-99)
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                                    "SELECT * FROM bundle WHERE filter_value < 1",
                                    vec![],
                                    None,
                                )
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_filter_broad(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("filter_broad");

    for format in ALL_FORMATS {
        // 50% selectivity filter: filter_value < 50
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                                    "SELECT * FROM bundle WHERE filter_value < 50",
                                    vec![],
                                    None,
                                )
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_aggregation_sum(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("aggregation_sum");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                                    None,
                                )
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_filter_parameterized(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("filter_parameterized");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                                    "SELECT * FROM bundle WHERE filter_value < $1",
                                    vec![ScalarValue::Int64(Some(50))],
                                    None,
                                )
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_join_small_large(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("join_small_large");

    for format in ALL_FORMATS {
        // Join 1K lookup table with 10K-100K main table
        for main_rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(main_rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format.name(), format!("1k_with_{}", main_rows)),
                &main_rows,
                |b, &main_rows| {
                    let main_url = bench_data::get_data_url(main_rows, &format);
                    let lookup_url = bench_data::get_lookup_url(SCALE_1K, &format);
                    let bundle = rt.block_on(async {
                        let url = fresh_dir("bundle");
                        let bundle = BundleBuilder::create(&url, None)
                            .await
                            .expect("bundle creation failed");
                        bundle.attach(&main_url, None).await.expect("attach failed");
                        bundle.commit("Setup").await.expect("commit failed");
                        bundle
                    });

                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
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
                            let mut stream = df.as_ref().clone().execute_stream().await.expect("stream failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_projection(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("projection");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                                .query("SELECT id, category, amount FROM bundle", vec![], None)
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

/// Benchmark planning phase only: parse SQL and build logical plan.
fn bench_plan_only(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("plan_only");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                            let ctx = bundle.ctx();
                            let _plan = ctx
                                .state()
                                .create_logical_plan(
                                    "SELECT * FROM bundle WHERE filter_value < 1",
                                )
                                .await
                                .expect("planning failed");
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

/// Benchmark execution phase only: run a pre-built plan and consume results.
fn bench_execute_only(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("execute_only");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
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
                            let ctx = bundle.ctx();
                            // Plan
                            let plan = ctx
                                .state()
                                .create_logical_plan(
                                    "SELECT * FROM bundle WHERE filter_value < 1",
                                )
                                .await
                                .expect("planning failed");

                            // Execute + consume
                            let df = ctx
                                .execute_logical_plan(plan)
                                .await
                                .expect("execute failed");
                            let mut stream =
                                df.execute_stream().await.expect("stream failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
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
    bench_plan_only,
    bench_execute_only,
);

bench_helpers::bench_main!(benches);
