//! Bundle lifecycle benchmarks
//!
//! Benchmarks for create, open, attach, and commit operations.
//! All data is written to disk under $TMPDIR/bundlebase/ (cleaned per run).

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::ALL_FORMATS;
use bench_helpers::{create_runtime, fresh_dir};
use bundlebase::BundleBuilder;
use bundlebase_command::BundleBuilderExt;
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_10K, SCALE_1K};

fn bench_create_bundle(c: &mut Criterion) {
    let rt = create_runtime();

    c.bench_function("create_empty_bundle", |b| {
        b.to_async(&rt).iter(|| async {
            let url = fresh_dir("bundle");
            let bundle = BundleBuilder::create(&url, None)
                .await
                .expect("bundle creation failed");
            bundle
                .commit("Created bundle")
                .await
                .expect("Commit failed");
            drop(bundle);
        });
    });
}

fn bench_attach_data(c: &mut Criterion) {
    let rt = create_runtime();

    let mut group = c.benchmark_group("attach_data");
    group.sample_size(10);

    for format in ALL_FORMATS {
        for rows in [SCALE_1K, SCALE_10K] {
            let data_url = bench_data::get_data_url(rows, &format);

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new(format.name(), rows), &rows, |b, &_rows| {
                let data_url = data_url.clone();
                b.to_async(&rt).iter(|| {
                    let data_url = data_url.clone();
                    async move {
                        let url = fresh_dir("bundle");
                        let bundle = BundleBuilder::create(&url, None)
                            .await
                            .expect("bundle creation failed");
                        bundle.attach(&data_url, None).await.expect("attach failed");
                        bundle.commit("Attached file").await.expect("commit failed");
                        drop(bundle);
                    }
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_create_bundle, bench_attach_data,);

bench_helpers::bench_main!(benches);
