//! Command operations benchmarks
//!
//! Benchmarks for bundle mutation commands: commit, permanent filter,
//! replace_block, reset, undo, verify_data, rebuild_index, reindex,
//! drop_column, join creation, and fetch.

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::{Format, ALL_FORMATS};
use bench_helpers::{create_benchmark_bundle, create_runtime, fresh_dir};
use bundlebase::bundle::BundleFacade;
use bundlebase::source::SyncMode;
use bundlebase::{BundleBuilder, BundlebaseError, JoinTypeOption};
use bundlebase_command::BundleBuilderExt;
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_100K, SCALE_10K, SCALE_1K};
use std::collections::HashMap;
use std::sync::Arc;

/// Create a bundle with data attached but NOT committed.
async fn create_uncommitted_bundle(
    rows: usize,
    format: &Format,
) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let data_url = bench_data::get_data_url(rows, format);
    let url = fresh_dir("bundle");
    let bundle = BundleBuilder::create(&url, None).await?;
    bundle.attach(&data_url, None).await?;
    Ok(bundle)
}

/// Create a committed bundle with a column index on 'id'.
async fn create_indexed_committed_bundle(
    rows: usize,
    format: &Format,
) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let bundle = create_benchmark_bundle(rows, format).await?;
    bundle.rebuild_index("id").await?;
    Ok(bundle)
}

/// Create a committed bundle with column indexes on 'id' AND 'filter_value'.
async fn create_multi_indexed_bundle(
    rows: usize,
    format: &Format,
) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    let bundle = create_benchmark_bundle(rows, format).await?;
    bundle.rebuild_index("id").await?;
    bundle.rebuild_index("filter_value").await?;
    Ok(bundle)
}

fn bench_commit(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("commit");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                setup_rt
                                    .block_on(create_uncommitted_bundle(rows, &format))
                                    .expect("bundle creation")
                            })
                            .join()
                            .expect("setup thread panicked")
                        })
                    },
                    |bundle| async move {
                        bundle
                            .commit("Benchmark commit")
                            .await
                            .expect("commit failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_filter_permanent_selective(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("filter_permanent_1pct");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
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
                            .filter("SELECT * FROM bundle WHERE filter_value < 1", vec![])
                            .await
                            .expect("filter failed");
                        bundle.commit("Filtered").await.expect("commit failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_filter_permanent_broad(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("filter_permanent_50pct");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
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
                            .filter("SELECT * FROM bundle WHERE filter_value < 50", vec![])
                            .await
                            .expect("filter failed");
                        bundle.commit("Filtered").await.expect("commit failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_replace_block(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("replace_block");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                let new_data_url = bench_data::get_data_url(rows, &format);

                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            let new_data_url = new_data_url.clone();
                            s.spawn(move || {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                let bundle = setup_rt
                                    .block_on(create_benchmark_bundle(rows, &format))
                                    .expect("bundle creation");
                                // Extract the URL of the first block in the base pack
                                let packs = bundle.packs();
                                let base_pack = packs.values().next().expect("no packs found");
                                let blocks = base_pack.blocks();
                                let first_block = blocks.first().expect("no blocks found");
                                let old_url = first_block.reader().url().to_string();
                                (bundle, old_url, new_data_url)
                            })
                            .join()
                            .expect("setup thread panicked")
                        })
                    },
                    |(bundle, old_url, new_url)| async move {
                        bundle
                            .replace_block(&old_url, &new_url)
                            .await
                            .expect("replace_block failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_reset(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("reset");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                let bundle = setup_rt
                                    .block_on(create_benchmark_bundle(rows, &format))
                                    .expect("bundle creation");
                                // Apply a filter to create uncommitted state
                                setup_rt
                                    .block_on(bundle.filter(
                                        "SELECT * FROM bundle WHERE filter_value < 50",
                                        vec![],
                                    ))
                                    .expect("filter failed");
                                bundle
                            })
                            .join()
                            .expect("setup thread panicked")
                        })
                    },
                    |bundle| async move {
                        bundle.reset().await.expect("reset failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_undo(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("undo");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                let bundle = setup_rt
                                    .block_on(create_benchmark_bundle(rows, &format))
                                    .expect("bundle creation");
                                // Apply a filter to create undoable state
                                setup_rt
                                    .block_on(bundle.filter(
                                        "SELECT * FROM bundle WHERE filter_value < 50",
                                        vec![],
                                    ))
                                    .expect("filter failed");
                                bundle
                            })
                            .join()
                            .expect("setup thread panicked")
                        })
                    },
                    |bundle| async move {
                        bundle.undo().await.expect("undo failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_verify_data(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("verify_data");

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K, SCALE_100K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                let bundle = rt
                    .block_on(create_benchmark_bundle(rows, &format))
                    .expect("bundle creation");

                b.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move {
                        bundle.verify_data(false).await.expect("verify_data failed");
                    }
                });
            });
        }
    }
    group.finish();
}

fn bench_rebuild_index(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("rebuild_index_cmd");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                setup_rt
                                    .block_on(create_indexed_committed_bundle(rows, &format))
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
                            .expect("rebuild_index failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_reindex(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("reindex");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let setup_rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("setup runtime");
                                setup_rt
                                    .block_on(create_multi_indexed_bundle(rows, &format))
                                    .expect("bundle creation")
                            })
                            .join()
                            .expect("setup thread panicked")
                        })
                    },
                    |bundle| async move {
                        bundle.reindex().await.expect("reindex failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_drop_column(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("drop_column");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                b.to_async(&rt).iter_batched(
                    || {
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
                            .drop_column("region")
                            .await
                            .expect("drop_column failed");
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_join_create(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("join_create");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            let lookup_url = bench_data::get_lookup_url(SCALE_1K, &format);
            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &rows| {
                let lookup_url = lookup_url.clone();
                b.to_async(&rt).iter_batched(
                    || {
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
                    |bundle| {
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
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_fetch(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("fetch");
    group.sample_size(10);

    // Fetch benchmark only uses parquet to keep manageable
    for rows in [SCALE_1K, SCALE_10K] {
        let format = Format::Parquet;
        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("parquet", rows), &rows, |b, &rows| {
            // Pre-generate the data file
            let data_url = bench_data::get_data_url(rows, &format);

            b.to_async(&rt).iter_batched(
                || {
                    std::thread::scope(|s| {
                        let data_url = data_url.clone();
                        s.spawn(move || {
                            let setup_rt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("setup runtime");

                            // Create a source directory with data files
                            let source_dir = bench_helpers::bench_tmp_dir()
                                .join(format!("fetch_source_{}", rand::random::<u64>()));
                            std::fs::create_dir_all(&source_dir)
                                .expect("failed to create source dir");

                            // Copy data file into the source directory
                            let data_path = url::Url::parse(&data_url)
                                .expect("parse data url")
                                .to_file_path()
                                .expect("to file path");
                            let dest_file =
                                source_dir.join(data_path.file_name().expect("file name"));
                            std::fs::copy(&data_path, &dest_file).expect("copy data file");

                            let source_url = url::Url::from_file_path(&source_dir)
                                .expect("source url")
                                .to_string();

                            // Create bundle with source pointing to the dir,
                            // but commit before the data is fetched
                            let bundle_url = fresh_dir("bundle");
                            let bundle = setup_rt
                                .block_on(async {
                                    let bundle = BundleBuilder::create(&bundle_url, None).await?;
                                    let mut args = HashMap::new();
                                    args.insert("url".to_string(), format!("{}/", source_url));
                                    bundle.create_source("remote_dir", args, None).await?;
                                    bundle.commit("Setup with source").await?;
                                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(bundle)
                                })
                                .expect("bundle setup");

                            bundle
                        })
                        .join()
                        .expect("setup thread panicked")
                    })
                },
                |bundle| async move {
                    bundle
                        .fetch("base", SyncMode::Add)
                        .await
                        .expect("fetch failed");
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_commit,
    bench_filter_permanent_selective,
    bench_filter_permanent_broad,
    bench_replace_block,
    bench_reset,
    bench_undo,
    bench_verify_data,
    bench_rebuild_index,
    bench_reindex,
    bench_drop_column,
    bench_join_create,
    bench_fetch,
);

bench_helpers::bench_main!(benches);
