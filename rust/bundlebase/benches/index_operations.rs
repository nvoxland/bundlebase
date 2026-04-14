//! Index operations benchmarks
//!
//! Benchmarks for index creation, lookup, and comparison with full scans.

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::{Format, ALL_FORMATS};
use bench_helpers::{create_benchmark_bundle, create_runtime};
use bundlebase::bundle::BundleFacade;
use bundlebase::{BundleBuilder, BundlebaseError};
use bundlebase_command::BundleBuilderExt;
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K, SCALE_1K};
use futures::StreamExt;
use std::sync::Arc;

/// Create a bundle with an index already built on the 'id' column
async fn create_indexed_bundle(
    rows: usize,
    format: &Format,
) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let bundle = create_benchmark_bundle(rows, format).await?;
    bundle.rebuild_index("id").await?;
    Ok(bundle)
}

fn bench_create_index(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("create_index");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
                        // Spawn a separate thread for setup because iter_batched's
                        // setup closure runs inside the async runtime context,
                        // and block_on cannot be called from within a runtime.
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                setup_rt
                                    .block_on(create_benchmark_bundle(rows, &format))
                                    .expect("bundle creation")
                            })
                            .join()
                            .expect("setup thread panicked")
                        })
                    },
                    |bundle| async move {
                        bundle
                            .rebuild_index("id")
                            .await
                            .expect("index creation failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_index_lookup_exact(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("index_lookup_exact");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.bench_with_input(
                BenchmarkId::new(format.name(), format!("{}_rows", rows)),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_indexed_bundle(rows, &format))
                        .expect("bundle creation");
                    let target_id = (rows / 2) as i64;

                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    &format!("SELECT * FROM bundle WHERE id = {}", target_id),
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

fn bench_index_vs_scan(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("index_vs_scan");

    let rows = SCALE_100K;

    for format in ALL_FORMATS {
        let indexed_bundle = rt
            .block_on(create_indexed_bundle(rows, &format))
            .expect("indexed bundle creation");
        let unindexed_bundle = rt
            .block_on(create_benchmark_bundle(rows, &format))
            .expect("unindexed bundle creation");

        let target_id = (rows / 2) as i64;

        group.bench_with_input(
            BenchmarkId::new(format!("indexed_{}", format.name()), "100k"),
            &rows,
            |b, _| {
                b.to_async(&rt).iter(|| {
                    let bundle = indexed_bundle.clone();
                    async move {
                        let mut stream = bundle
                            .query(
                                &format!("SELECT * FROM bundle WHERE id = {}", target_id),
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

        group.bench_with_input(
            BenchmarkId::new(format!("scan_{}", format.name()), "100k"),
            &rows,
            |b, _| {
                b.to_async(&rt).iter(|| {
                    let bundle = unindexed_bundle.clone();
                    async move {
                        let mut stream = bundle
                            .query(
                                &format!("SELECT * FROM bundle WHERE id = {}", target_id),
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

    group.finish();
}

fn bench_index_range_query(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("index_range_query");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.bench_with_input(
                BenchmarkId::new(format.name(), format!("{}_rows", rows)),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_indexed_bundle(rows, &format))
                        .expect("bundle creation");
                    let min_id = (rows / 10) as i64;
                    let max_id = (rows / 5) as i64;

                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    &format!(
                                        "SELECT * FROM bundle WHERE id >= {} AND id < {}",
                                        min_id, max_id
                                    ),
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

fn bench_index_in_query(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("index_in_query");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.bench_with_input(
                BenchmarkId::new(format.name(), format!("{}_rows", rows)),
                &rows,
                |b, &rows| {
                    let bundle = rt
                        .block_on(create_indexed_bundle(rows, &format))
                        .expect("bundle creation");
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
                            let mut stream = bundle
                                .query(
                                    &format!("SELECT * FROM bundle WHERE id IN ({})", id_list),
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

criterion_group!(
    benches,
    bench_create_index,
    bench_index_lookup_exact,
    bench_index_vs_scan,
    bench_index_range_query,
    bench_index_in_query,
);

bench_helpers::bench_main!(benches);
