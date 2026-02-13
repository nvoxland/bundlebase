//! Index operations benchmarks
//!
//! Benchmarks for index creation, lookup, and comparison with full scans.

mod bench_data;
mod data_generator;

use bench_data::Format;
use bundlebase::bundle::BundleFacade;
use bundlebase::{BundleBuilder, BundlebaseError};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K, SCALE_1K};
use std::sync::Arc;
use tokio::runtime::Runtime;
use url::Url;

fn random_memory_url() -> Url {
    Url::parse(&format!("memory:///bench/{}", rand::random::<u64>())).expect("valid url")
}

/// Create a bundle with synthetic data
async fn create_benchmark_bundle(rows: usize) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let data_url = bench_data::get_data_url(rows, &Format::Parquet);

    let bundle_url = random_memory_url();
    let bundle = BundleBuilder::create(bundle_url.as_str(), None).await?;
    bundle.attach(&data_url, None).await?;

    Ok(bundle)
}

/// Create a bundle with an index already built on the 'id' column
async fn create_indexed_bundle(rows: usize) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let bundle = create_benchmark_bundle(rows).await?;
    bundle.rebuild_index("id").await?;
    Ok(bundle)
}

fn bench_create_index(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("create_index");

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::from_parameter(rows), &rows, |b, &rows| {
            b.to_async(&rt).iter_batched(
                || {
                    // Setup: create bundle without index
                    rt.block_on(create_benchmark_bundle(rows)).expect("bundle creation")
                },
                |bundle| async move {
                    bundle.rebuild_index("id").await.expect("index creation failed");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_index_lookup_exact(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("index_lookup_exact");

    // Benchmark indexed equality lookup
    for rows in [SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_rows", rows)),
            &rows,
            |b, &rows| {
                let bundle = rt.block_on(create_indexed_bundle(rows)).expect("bundle creation");

                // Lookup a specific ID in the middle of the range
                let target_id = (rows / 2) as i64;

                b.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move {
                        bundle
                            .filter(&format!("SELECT * FROM bundle WHERE id = {}", target_id), vec![])
                            .await
                            .expect("filter failed");
                        let df = bundle.dataframe().await.expect("dataframe failed");
                        let _result = df.as_ref().clone().collect().await.expect("collect failed");
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_index_vs_scan(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("index_vs_scan");

    let rows = SCALE_100K;

    // Create two bundles: one with index, one without
    let indexed_bundle = rt
        .block_on(create_indexed_bundle(rows))
        .expect("indexed bundle creation");
    let unindexed_bundle = rt
        .block_on(create_benchmark_bundle(rows))
        .expect("unindexed bundle creation");

    let target_id = (rows / 2) as i64;

    // Benchmark indexed lookup
    group.bench_function("indexed_100k", |b| {
        b.to_async(&rt).iter(|| {
            let bundle = indexed_bundle.clone();
            async move {
                bundle
                    .filter(&format!("SELECT * FROM bundle WHERE id = {}", target_id), vec![])
                    .await
                    .expect("filter failed");
                let df = bundle.dataframe().await.expect("dataframe failed");
                let _result = df.as_ref().clone().collect().await.expect("collect failed");
            }
        });
    });

    // Benchmark full scan
    group.bench_function("scan_100k", |b| {
        b.to_async(&rt).iter(|| {
            let bundle = unindexed_bundle.clone();
            async move {
                bundle
                    .filter(&format!("SELECT * FROM bundle WHERE id = {}", target_id), vec![])
                    .await
                    .expect("filter failed");
                let df = bundle.dataframe().await.expect("dataframe failed");
                let _result = df.as_ref().clone().collect().await.expect("collect failed");
            }
        });
    });

    group.finish();
}

fn bench_index_range_query(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("index_range_query");

    for rows in [SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_rows", rows)),
            &rows,
            |b, &rows| {
                let bundle = rt.block_on(create_indexed_bundle(rows)).expect("bundle creation");

                // Range query: select 10% of rows
                let min_id = (rows / 10) as i64;
                let max_id = (rows / 5) as i64;

                b.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move {
                        bundle
                            .filter(
                                &format!("SELECT * FROM bundle WHERE id >= {} AND id < {}", min_id, max_id),
                                vec![],
                            )
                            .await
                            .expect("filter failed");
                        let df = bundle.dataframe().await.expect("dataframe failed");
                        let _result = df.as_ref().clone().collect().await.expect("collect failed");
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_index_in_query(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("index_in_query");

    for rows in [SCALE_10K, SCALE_100K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_rows", rows)),
            &rows,
            |b, &rows| {
                let bundle = rt.block_on(create_indexed_bundle(rows)).expect("bundle creation");

                // IN query: select specific IDs (10 values)
                let ids: Vec<i64> = (0..10).map(|i| (rows * i / 10) as i64).collect();
                let id_list = ids
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");

                b.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    let id_list = id_list.clone();
                    async move {
                        bundle
                            .filter(&format!("SELECT * FROM bundle WHERE id IN ({})", id_list), vec![])
                            .await
                            .expect("filter failed");
                        let df = bundle.dataframe().await.expect("dataframe failed");
                        let _result = df.as_ref().clone().collect().await.expect("collect failed");
                    }
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_create_index,
    bench_index_lookup_exact,
    bench_index_vs_scan,
    bench_index_range_query,
    bench_index_in_query,
);
criterion_main!(benches);
