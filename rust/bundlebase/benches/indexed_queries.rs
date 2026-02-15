//! Indexed query pattern benchmarks
//!
//! Benchmarks comparing query performance with and without indexes.
//! Tests ORDER BY, LIMIT, DISTINCT, GROUP BY, aggregations, LIKE,
//! and text_search patterns.

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::ALL_FORMATS;
use bench_helpers::{create_benchmark_bundle, create_runtime};
use bundlebase::bundle::BundleFacade;
use bundlebase::{BundleBuilder, BundlebaseError};
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K};
use futures::StreamExt;
use std::sync::Arc;

/// Create a committed bundle with a column index on the specified column.
async fn create_bundle_with_column_index(
    rows: usize,
    format: &bench_data::Format,
    column: &str,
) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let bundle = create_benchmark_bundle(rows, format).await?;
    bundle.rebuild_index(column).await?;
    Ok(bundle)
}


fn bench_order_by(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("order_by");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));

            // No index
            let no_index_bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("no_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = no_index_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT * FROM bundle ORDER BY id LIMIT 1000",
                                    vec![],
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

            // Column index on id
            let indexed_bundle = rt
                .block_on(create_bundle_with_column_index(rows, &format, "id"))
                .expect("indexed bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("column_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = indexed_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT * FROM bundle ORDER BY id LIMIT 1000",
                                    vec![],
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

fn bench_limit_with_filter(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("limit_with_filter");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));

            // No index
            let no_index_bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("no_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = no_index_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT * FROM bundle WHERE filter_value < 50 ORDER BY id LIMIT 100",
                                    vec![],
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

            // Column index on filter_value
            let indexed_bundle = rt
                .block_on(create_bundle_with_column_index(rows, &format, "filter_value"))
                .expect("indexed bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("column_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = indexed_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT * FROM bundle WHERE filter_value < 50 ORDER BY id LIMIT 100",
                                    vec![],
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

fn bench_distinct(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("distinct");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));

            // No index
            let no_index_bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("no_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = no_index_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT DISTINCT category FROM bundle",
                                    vec![],
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

            // Column index on category
            let indexed_bundle = rt
                .block_on(create_bundle_with_column_index(rows, &format, "category"))
                .expect("indexed bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("column_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = indexed_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT DISTINCT category FROM bundle",
                                    vec![],
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

fn bench_count_group_by(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("count_group_by");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));

            // No index
            let no_index_bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("no_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = no_index_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT category, COUNT(*) FROM bundle GROUP BY category",
                                    vec![],
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

            // Column index on category
            let indexed_bundle = rt
                .block_on(create_bundle_with_column_index(rows, &format, "category"))
                .expect("indexed bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("column_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = indexed_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT category, COUNT(*) FROM bundle GROUP BY category",
                                    vec![],
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

fn bench_aggregations(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("aggregations");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));

            let bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");

            // COUNT(*)
            group.bench_with_input(
                BenchmarkId::new(format!("count_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT COUNT(*) FROM bundle", vec![])
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );

            // AVG(amount)
            group.bench_with_input(
                BenchmarkId::new(format!("avg_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT AVG(amount) FROM bundle", vec![])
                                .await
                                .expect("query failed");
                            while let Some(batch_result) = stream.next().await {
                                let _batch = batch_result.expect("batch failed");
                            }
                        }
                    });
                },
            );

            // MIN/MAX
            group.bench_with_input(
                BenchmarkId::new(format!("min_max_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT MIN(amount), MAX(amount) FROM bundle",
                                    vec![],
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

fn bench_like(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("like_query");

    for format in ALL_FORMATS {
        for rows in [SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));

            // No index
            let no_index_bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("no_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = no_index_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT * FROM bundle WHERE name LIKE 'item_0000%'",
                                    vec![],
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

            // Column index on name
            let indexed_bundle = rt
                .block_on(create_bundle_with_column_index(rows, &format, "name"))
                .expect("indexed bundle creation");
            group.bench_with_input(
                BenchmarkId::new(format!("column_index_{}", format.name()), rows),
                &rows,
                |b, _| {
                    b.to_async(&rt).iter(|| {
                        let bundle = indexed_bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query(
                                    "SELECT * FROM bundle WHERE name LIKE 'item_0000%'",
                                    vec![],
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
    bench_order_by,
    bench_limit_with_filter,
    bench_distinct,
    bench_count_group_by,
    bench_aggregations,
    bench_like,
);

bench_helpers::bench_main!(benches);
