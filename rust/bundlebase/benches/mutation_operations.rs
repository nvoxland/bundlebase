//! Benchmarks for DELETE, UPDATE, and combined mutation operations.
//!
//! Measures performance of:
//! - DELETE with various selectivity levels
//! - UPDATE with single and multiple columns
//! - Combined DELETE + UPDATE workflows
//! - Querying data after mutations (overlay scan cost)
//! - Commit + reopen round-trip with mutations

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::Format;
use bench_helpers::{create_benchmark_bundle, create_runtime};
use bundlebase::bundle::BundleFacade;
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_command::BundleBuilderExt;
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K, SCALE_1K};
use futures::StreamExt;
use std::sync::Arc;

/// Helper: create a bundle setup in a separate thread (avoids runtime nesting in iter_batched).
fn setup_bundle(_rt: &tokio::runtime::Runtime, rows: usize, format: &Format) -> Arc<BundleBuilder> {
    std::thread::scope(|s| {
        s.spawn(|| {
            let setup_rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("setup runtime");
            setup_rt
                .block_on(create_benchmark_bundle(rows, format))
                .expect("bundle creation")
        })
        .join()
        .expect("setup thread panicked")
    })
}

// ---------------------------------------------------------------------------
// DELETE benchmarks
// ---------------------------------------------------------------------------

fn bench_delete(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("delete");
    group.sample_size(10);

    let format = Format::Parquet;

    // Vary selectivity: ~1%, ~10%, ~50% of rows
    for (label, where_clause) in [
        ("1pct", "filter_value < 1"),
        ("10pct", "filter_value < 10"),
        ("50pct", "filter_value < 50"),
    ] {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}_{}", label, format.name()), rows),
                &rows,
                |b, &rows| {
                    b.to_async(&rt).iter_batched(
                        || setup_bundle(&rt, rows, &format),
                        |bundle| {
                            let wc = where_clause.to_string();
                            async move {
                                bundle.delete(&wc).await.expect("delete failed");
                            }
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    group.finish();
}

fn bench_delete_commit(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("delete_commit");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
            b.to_async(&rt).iter_batched(
                || setup_bundle(&rt, rows, &format),
                |bundle| async move {
                    bundle
                        .delete("filter_value < 10")
                        .await
                        .expect("delete failed");
                    bundle.commit("Deleted rows").await.expect("commit failed");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// UPDATE benchmarks
// ---------------------------------------------------------------------------

fn bench_update(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("update");
    group.sample_size(10);

    let format = Format::Parquet;

    // Single column update with varying selectivity
    for (label, set_where) in [
        ("1col_1pct", "SET amount = 0 WHERE filter_value < 1"),
        ("1col_10pct", "SET amount = 0 WHERE filter_value < 10"),
        ("1col_50pct", "SET amount = 0 WHERE filter_value < 50"),
    ] {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}_{}", label, format.name()), rows),
                &rows,
                |b, &rows| {
                    b.to_async(&rt).iter_batched(
                        || setup_bundle(&rt, rows, &format),
                        |bundle| {
                            let sw = set_where.to_string();
                            async move {
                                bundle.update(&sw).await.expect("update failed");
                            }
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    // Multi-column update
    for rows in [SCALE_1K, SCALE_10K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(
            BenchmarkId::new(format!("3col_10pct_{}", format.name()), rows),
            &rows,
            |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || setup_bundle(&rt, rows, &format),
                    |bundle| async move {
                        bundle
                            .update("SET amount = 0, category = 'X', region = 'Unknown' WHERE filter_value < 10")
                            .await
                            .expect("update failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_update_commit(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("update_commit");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
            b.to_async(&rt).iter_batched(
                || setup_bundle(&rt, rows, &format),
                |bundle| async move {
                    bundle
                        .update("SET amount = amount * 1.1 WHERE filter_value < 10")
                        .await
                        .expect("update failed");
                    bundle.commit("Updated rows").await.expect("commit failed");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Query-after-mutation benchmarks (measures overlay scan cost)
// ---------------------------------------------------------------------------

fn bench_query_after_delete(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("query_after_delete");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        // Baseline: query without delete
        {
            let bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("baseline_{}", format.name()), rows),
                &rows,
                |b, &_rows| {
                    let bundle = bundle.clone();
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT * FROM bundle", vec![], None)
                                .await
                                .expect("query failed");
                            while let Some(batch) = stream.next().await {
                                let _ = batch.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }

        // Query after 10% delete (committed + reopened)
        {
            let bundle = rt.block_on(async {
                let b = create_benchmark_bundle(rows, &format)
                    .await
                    .expect("bundle");
                b.delete("filter_value < 10").await.expect("delete");
                b.commit("Deleted").await.expect("commit");
                Bundle::open(b.url().as_str(), None).await.expect("reopen")
            });

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("after_delete_{}", format.name()), rows),
                &rows,
                |b, &_rows| {
                    let bundle = bundle.clone();
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT * FROM bundle", vec![], None)
                                .await
                                .expect("query failed");
                            while let Some(batch) = stream.next().await {
                                let _ = batch.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_query_after_update(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("query_after_update");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        // Baseline: query without update
        {
            let bundle = rt
                .block_on(create_benchmark_bundle(rows, &format))
                .expect("bundle creation");

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("baseline_{}", format.name()), rows),
                &rows,
                |b, &_rows| {
                    let bundle = bundle.clone();
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT * FROM bundle", vec![], None)
                                .await
                                .expect("query failed");
                            while let Some(batch) = stream.next().await {
                                let _ = batch.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }

        // Query after 10% update (committed + reopened)
        {
            let bundle = rt.block_on(async {
                let b = create_benchmark_bundle(rows, &format)
                    .await
                    .expect("bundle");
                b.update("SET amount = amount * 1.1 WHERE filter_value < 10")
                    .await
                    .expect("update");
                b.commit("Updated").await.expect("commit");
                Bundle::open(b.url().as_str(), None).await.expect("reopen")
            });

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("after_update_{}", format.name()), rows),
                &rows,
                |b, &_rows| {
                    let bundle = bundle.clone();
                    b.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move {
                            let mut stream = bundle
                                .query("SELECT * FROM bundle", vec![], None)
                                .await
                                .expect("query failed");
                            while let Some(batch) = stream.next().await {
                                let _ = batch.expect("batch failed");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Combined DELETE + UPDATE benchmarks
// ---------------------------------------------------------------------------

fn bench_delete_then_update(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("delete_then_update");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
            b.to_async(&rt).iter_batched(
                || setup_bundle(&rt, rows, &format),
                |bundle| async move {
                    // Delete negative-sentinel rows, then update remaining
                    bundle
                        .delete("filter_value < 5")
                        .await
                        .expect("delete failed");
                    bundle
                        .update(
                            "SET amount = amount * 2 WHERE filter_value >= 5 AND filter_value < 15",
                        )
                        .await
                        .expect("update failed");
                    bundle
                        .commit("Delete + Update")
                        .await
                        .expect("commit failed");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_query_after_delete_and_update(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("query_after_delete_and_update");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
        // Query after both delete and update (committed + reopened)
        let bundle = rt.block_on(async {
            let b = create_benchmark_bundle(rows, &format)
                .await
                .expect("bundle");
            b.delete("filter_value < 5").await.expect("delete");
            b.update("SET amount = 0 WHERE filter_value >= 5 AND filter_value < 15")
                .await
                .expect("update");
            b.commit("Delete + Update").await.expect("commit");
            let reopened = Bundle::open(b.url().as_str(), None).await.expect("reopen");
            Arc::new(reopened)
        });

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query("SELECT * FROM bundle", vec![], None)
                        .await
                        .expect("query failed");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch failed");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Reopen cost benchmarks
// ---------------------------------------------------------------------------

fn bench_reopen_with_mutations(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("reopen_with_mutations");
    group.sample_size(10);

    let format = Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        // Baseline: reopen without mutations
        {
            let url = rt.block_on(async {
                let b = create_benchmark_bundle(rows, &format)
                    .await
                    .expect("bundle");
                b.url().to_string()
            });

            group.bench_with_input(
                BenchmarkId::new(format!("baseline_{}", format.name()), rows),
                &rows,
                |b, &_rows| {
                    let url = url.clone();
                    b.to_async(&rt).iter(|| {
                        let url = url.clone();
                        async move {
                            let _ = Bundle::open(&url, None).await.expect("reopen");
                        }
                    });
                },
            );
        }

        // Reopen with delete + update committed
        {
            let url = rt.block_on(async {
                let b = create_benchmark_bundle(rows, &format)
                    .await
                    .expect("bundle");
                b.delete("filter_value < 10").await.expect("delete");
                b.update("SET amount = 0 WHERE filter_value >= 10 AND filter_value < 20")
                    .await
                    .expect("update");
                b.commit("Mutations").await.expect("commit");
                b.url().to_string()
            });

            group.bench_with_input(
                BenchmarkId::new(format!("with_mutations_{}", format.name()), rows),
                &rows,
                |b, &_rows| {
                    let url = url.clone();
                    b.to_async(&rt).iter(|| {
                        let url = url.clone();
                        async move {
                            let _ = Bundle::open(&url, None).await.expect("reopen");
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
    bench_delete,
    bench_delete_commit,
    bench_update,
    bench_update_commit,
    bench_query_after_delete,
    bench_query_after_update,
    bench_delete_then_update,
    bench_query_after_delete_and_update,
    bench_reopen_with_mutations,
);

bench_helpers::bench_main!(benches);
