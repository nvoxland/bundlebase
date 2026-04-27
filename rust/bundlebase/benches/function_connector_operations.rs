//! Benchmarks for UDF functions (FFI, Python IPC, Go IPC, Java IPC) and HTTP connectors.
//!
//! Measures:
//! - Function import + query overhead per runtime (FFI vs Python/Go/Java IPC vs native SQL)
//! - Scalar and aggregate function invocation cost
//! - HTTP connector create_source + fetch performance
//! - Throttled HTTP connector (simulating remote API latency)
//!
//! Prerequisites:
//! - FFI: `cd tests/test_lib_function && cargo build`
//! - Python: `pip install bundlebase_sdk`
//! - Go: `cd benches/bench_functions && go build -o double_val_go double_val.go`
//! - Java: `cd sdk/java && mvn package -q` then compile DoubleVal.java

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_helpers::{create_benchmark_bundle, create_local_benchmark_bundle, create_runtime};
use bundlebase::bundle::BundleFacade;
use bundlebase::BundleBuilder;
use bundlebase_command::{BundleBuilderExt, BundleFacadeCommandExt};
use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use data_generator::{SCALE_10K, SCALE_1K};
use futures::StreamExt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Path to the FFI test library (built separately via cargo build in test_lib_function/).
fn ffi_lib_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let lib_name = if cfg!(target_os = "macos") {
        "libtest_lib_function.dylib"
    } else {
        "libtest_lib_function.so"
    };
    format!(
        "{}/tests/test_lib_function/target/debug/{}",
        manifest_dir, lib_name
    )
}

/// Path to the Python IPC function script.
fn python_function_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/benches/bench_functions/double_val.py", manifest_dir)
}

/// Check if the FFI test library exists (must be built separately).
fn ffi_lib_exists() -> bool {
    std::path::Path::new(&ffi_lib_path()).exists()
}

/// Check if Python is available and the SDK is importable.
fn python_available() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import bundlebase_sdk"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Path to the Go IPC function binary (built separately).
fn go_function_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/benches/bench_functions/double_val_go", manifest_dir)
}

/// Check if the Go function binary exists.
fn go_available() -> bool {
    std::path::Path::new(&go_function_path()).exists()
}

/// Path to the Java IPC function wrapper script.
fn java_function_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!(
        "{}/benches/bench_functions/double_val_java.sh",
        manifest_dir
    )
}

/// Check if Java function is available (wrapper script exists and produces manifest).
fn java_available() -> bool {
    let path = java_function_path();
    if !std::path::Path::new(&path).exists() {
        return false;
    }
    std::process::Command::new(&path)
        .arg("--bundlebase-functions")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Setup a bundle with FFI function imported. Runs in a separate thread.
/// Uses local filesystem (not throttled) because FFI needs real dlopen paths.
fn setup_ffi_bundle(rows: usize, format: &bench_data::Format) -> Option<Arc<BundleBuilder>> {
    if !ffi_lib_exists() {
        eprintln!(
            "SKIP: FFI test library not built. Run: cd tests/test_lib_function && cargo build"
        );
        return None;
    }
    let ffi_path = ffi_lib_path();
    std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("setup runtime");
            rt.block_on(async {
                let bundle = create_local_benchmark_bundle(rows, format)
                    .await
                    .expect("bundle");
                bundle
                    .import_function("bench.double_val", &format!("ffi::{}", ffi_path), "*/*")
                    .await
                    .expect("import FFI function");
                bundle
            })
        })
        .join()
        .ok()
    })
}

/// Setup a bundle with Python IPC function imported. Runs in a separate thread.
/// Uses local filesystem because IPC subprocess needs real file paths.
fn setup_python_bundle(rows: usize, format: &bench_data::Format) -> Option<Arc<BundleBuilder>> {
    if !python_available() {
        eprintln!("SKIP: Python SDK not available. Install bundlebase_sdk.");
        return None;
    }
    let py_path = python_function_path();
    std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("setup runtime");
            rt.block_on(async {
                let bundle = create_local_benchmark_bundle(rows, format)
                    .await
                    .expect("bundle");
                bundle
                    .import_function("bench.double_val", &format!("python::{}", py_path), "*/*")
                    .await
                    .expect("import Python function");
                bundle
            })
        })
        .join()
        .ok()
    })
}

// ---------------------------------------------------------------------------
// Native SQL baseline
// ---------------------------------------------------------------------------

fn bench_native_sql(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_scalar");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        let bundle = rt
            .block_on(create_benchmark_bundle(rows, &format))
            .expect("bundle");

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("native_sql", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query("SELECT id * 2 AS doubled FROM bundle", vec![], None)
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// FFI function benchmarks
// ---------------------------------------------------------------------------

fn bench_ffi_scalar(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_scalar");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        let bundle = match setup_ffi_bundle(rows, &format) {
            Some(b) => b,
            None => continue,
        };

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("ffi", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query(
                            "SELECT bench.double_val(id) AS doubled FROM bundle",
                            vec![],
                            None,
                        )
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

fn bench_ffi_aggregate(c: &mut Criterion) {
    if !ffi_lib_exists() {
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_aggregate");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;
    let ffi_path = ffi_lib_path();

    for rows in [SCALE_1K, SCALE_10K] {
        let ffi_path = ffi_path.clone();
        let bundle = std::thread::scope(|s| {
            s.spawn(|| {
                let srt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("setup runtime");
                srt.block_on(async {
                    let b = create_local_benchmark_bundle(rows, &format)
                        .await
                        .expect("bundle");
                    b.import_function("bench.int_sum", &format!("ffi::{}", ffi_path), "*/*")
                        .await
                        .expect("import FFI aggregate");
                    b
                })
            })
            .join()
            .expect("setup thread")
        });

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("ffi", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query(
                            "SELECT bench.int_sum(id) AS total FROM bundle",
                            vec![],
                            None,
                        )
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Python IPC function benchmarks
// ---------------------------------------------------------------------------

fn bench_python_scalar(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_scalar");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        let bundle = match setup_python_bundle(rows, &format) {
            Some(b) => b,
            None => continue,
        };

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("python_ipc", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query(
                            "SELECT bench.double_val(id) AS doubled FROM bundle",
                            vec![],
                            None,
                        )
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Go IPC function benchmarks
// ---------------------------------------------------------------------------

fn bench_go_scalar(c: &mut Criterion) {
    if !go_available() {
        eprintln!("SKIP: Go function binary not built. Run: cd benches/bench_functions && go build -o double_val_go double_val.go");
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_scalar");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;
    let go_path = go_function_path();

    for rows in [SCALE_1K, SCALE_10K] {
        let go_path = go_path.clone();
        let bundle = std::thread::scope(|s| {
            s.spawn(|| {
                let srt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("setup runtime");
                srt.block_on(async {
                    let b = create_local_benchmark_bundle(rows, &format)
                        .await
                        .expect("bundle");
                    b.import_temp_function("bench.*", &format!("ipc::{}", go_path), "*/*")
                        .await
                        .expect("import Go function");
                    b
                })
            })
            .join()
            .expect("setup thread")
        });

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("go_ipc", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query(
                            "SELECT bench.double_val(id) AS doubled FROM bundle",
                            vec![],
                            None,
                        )
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Java IPC function benchmarks
// ---------------------------------------------------------------------------

fn bench_java_scalar(c: &mut Criterion) {
    if !java_available() {
        eprintln!("SKIP: Java function not available. Build: cd sdk/java && mvn package -q && compile DoubleVal.java");
        return;
    }
    let java_from = format!("ipc::{}", java_function_path());
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_scalar");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;

    for rows in [SCALE_1K, SCALE_10K] {
        let java_from = java_from.clone();
        let bundle = std::thread::scope(|s| {
            s.spawn(|| {
                let srt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("setup runtime");
                srt.block_on(async {
                    let b = create_local_benchmark_bundle(rows, &format).await.ok()?;
                    b.import_temp_function("bench.*", &java_from, "*/*")
                        .await
                        .ok()?;
                    Some(b)
                })
            })
            .join()
            .ok()
            .flatten()
        });

        let bundle = match bundle {
            Some(b) => b,
            None => {
                eprintln!("SKIP: Java function import failed (requires Java 22+)");
                continue;
            }
        };

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("java_ipc", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query(
                            "SELECT bench.double_val(id) AS doubled FROM bundle",
                            vec![],
                            None,
                        )
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// HTTP connector benchmarks
// ---------------------------------------------------------------------------

/// Start a mock HTTP server serving a parquet file. Returns (server_url, join_handle).
fn start_mock_http_server(
    data: bytes::Bytes,
    latency: std::time::Duration,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let url = format!("http://127.0.0.1:{}/data.parquet", addr.port());

    let handle = std::thread::spawn(move || {
        use std::io::{Read, Write};
        loop {
            let stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(_) => break,
            };
            let mut stream = stream;

            // Read HTTP request (we don't parse it, just drain it)
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);

            // Apply latency
            if !latency.is_zero() {
                std::thread::sleep(latency);
            }

            // Send HTTP response with parquet data
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                data.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&data);
            let _ = stream.flush();
        }
    });

    (url, handle)
}

/// Generate parquet bytes for the mock server.
fn generate_parquet_bytes(rows: usize) -> bytes::Bytes {
    use arrow::array::{Int64Array, RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    let ids: Vec<i64> = (0..rows as i64).collect();
    let names: Vec<String> = (0..rows).map(|i| format!("item_{}", i)).collect();
    let schema = std::sync::Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            std::sync::Arc::new(Int64Array::from(ids)),
            std::sync::Arc::new(StringArray::from(
                names.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            )),
        ],
    )
    .expect("batch");

    let mut buf = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, schema, None).expect("writer");
    writer.write(&batch).expect("write");
    writer.close().expect("close");
    bytes::Bytes::from(buf)
}

fn bench_http_connector(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("connector_http");
    group.sample_size(10);

    // Test with different data sizes and latencies
    for rows in [SCALE_1K, SCALE_10K] {
        let data = generate_parquet_bytes(rows);

        // No latency (local speed)
        {
            let (url, _handle) = start_mock_http_server(data.clone(), std::time::Duration::ZERO);

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new("no_latency", rows), &rows, |b, &rows| {
                let url = url.clone();
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let srt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("runtime");
                                throttled_store::register_throttle_scheme();
                                srt.block_on(create_benchmark_bundle(
                                    rows,
                                    &bench_data::Format::Parquet,
                                ))
                                .expect("bundle")
                            })
                            .join()
                            .expect("setup")
                        })
                    },
                    |bundle| {
                        let url = url.clone();
                        async move {
                            bundle
                                .create_source(
                                    "http",
                                    std::collections::HashMap::from([("url".to_string(), url)]),
                                    None,
                                    true,
                                )
                                .await
                                .expect("create_source");
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }

        // 50ms latency (simulating moderate API)
        {
            let (url, _handle) =
                start_mock_http_server(data.clone(), std::time::Duration::from_millis(50));

            group.throughput(Throughput::Elements(rows as u64));
            group.bench_with_input(BenchmarkId::new("50ms_latency", rows), &rows, |b, &rows| {
                let url = url.clone();
                b.to_async(&rt).iter_batched(
                    || {
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                let srt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("runtime");
                                throttled_store::register_throttle_scheme();
                                srt.block_on(create_benchmark_bundle(
                                    rows,
                                    &bench_data::Format::Parquet,
                                ))
                                .expect("bundle")
                            })
                            .join()
                            .expect("setup")
                        })
                    },
                    |bundle| {
                        let url = url.clone();
                        async move {
                            bundle
                                .create_source(
                                    "http",
                                    std::collections::HashMap::from([("url".to_string(), url)]),
                                    None,
                                    true,
                                )
                                .await
                                .expect("create_source");
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            });
        }
    }
    group.finish();
}

fn bench_http_connector_query(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("connector_http_query");
    group.sample_size(10);

    // Benchmark querying data fetched via HTTP connector
    for rows in [SCALE_1K, SCALE_10K] {
        let data = generate_parquet_bytes(rows);
        let (url, _handle) = start_mock_http_server(data, std::time::Duration::ZERO);

        let bundle = rt.block_on(async {
            let b = create_benchmark_bundle(rows, &bench_data::Format::Parquet)
                .await
                .expect("bundle");
            b.create_source(
                "http",
                std::collections::HashMap::from([("url".to_string(), url)]),
                None,
                true,
            )
            .await
            .expect("create_source");
            b
        });

        group.throughput(Throughput::Elements(rows as u64));
        group.bench_with_input(BenchmarkId::new("parquet", rows), &rows, |b, &_rows| {
            let bundle = bundle.clone();
            b.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move {
                    let mut stream = bundle
                        .query("SELECT * FROM bundle", vec![], None)
                        .await
                        .expect("query");
                    while let Some(batch) = stream.next().await {
                        let _ = batch.expect("batch");
                    }
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Function import cost
// ---------------------------------------------------------------------------

fn bench_function_import(c: &mut Criterion) {
    let rt = create_runtime();
    let mut group = c.benchmark_group("function_import");
    group.sample_size(10);

    let format = bench_data::Format::Parquet;

    // FFI import cost
    if ffi_lib_exists() {
        let ffi_path = ffi_lib_path();
        group.bench_function("ffi", |b| {
            let ffi_path = ffi_path.clone();
            b.to_async(&rt).iter_batched(
                || {
                    std::thread::scope(|s| {
                        s.spawn(|| {
                            let srt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("runtime");
                            srt.block_on(create_local_benchmark_bundle(SCALE_1K, &format))
                                .expect("bundle")
                        })
                        .join()
                        .expect("setup")
                    })
                },
                |bundle| {
                    let ffi_path = ffi_path.clone();
                    async move {
                        bundle
                            .import_function(
                                "bench.double_val",
                                &format!("ffi::{}", ffi_path),
                                "*/*",
                            )
                            .await
                            .expect("import");
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    // Python import cost
    if python_available() {
        let py_path = python_function_path();
        group.bench_function("python_ipc", |b| {
            let py_path = py_path.clone();
            b.to_async(&rt).iter_batched(
                || {
                    std::thread::scope(|s| {
                        s.spawn(|| {
                            let srt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("runtime");
                            srt.block_on(create_local_benchmark_bundle(SCALE_1K, &format))
                                .expect("bundle")
                        })
                        .join()
                        .expect("setup")
                    })
                },
                |bundle| {
                    let py_path = py_path.clone();
                    async move {
                        bundle
                            .import_function(
                                "bench.double_val",
                                &format!("python::{}", py_path),
                                "*/*",
                            )
                            .await
                            .expect("import");
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    // Go import cost
    if go_available() {
        let go_path = go_function_path();
        group.bench_function("go_ipc", |b| {
            let go_path = go_path.clone();
            b.to_async(&rt).iter_batched(
                || {
                    std::thread::scope(|s| {
                        s.spawn(|| {
                            let srt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("runtime");
                            srt.block_on(create_local_benchmark_bundle(SCALE_1K, &format))
                                .expect("bundle")
                        })
                        .join()
                        .expect("setup")
                    })
                },
                |bundle| {
                    let go_path = go_path.clone();
                    async move {
                        bundle
                            .import_temp_function("bench.*", &format!("ipc::{}", go_path), "*/*")
                            .await
                            .expect("import");
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    // Java import cost
    if java_available() {
        let java_from = format!("ipc::{}", java_function_path());
        group.bench_function("java_ipc", |b| {
            let java_from = java_from.clone();
            b.to_async(&rt).iter_batched(
                || {
                    std::thread::scope(|s| {
                        s.spawn(|| {
                            let srt = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .expect("runtime");
                            srt.block_on(create_local_benchmark_bundle(SCALE_1K, &format))
                                .expect("bundle")
                        })
                        .join()
                        .expect("setup")
                    })
                },
                |bundle| {
                    let java_from = java_from.clone();
                    async move {
                        bundle
                            .import_temp_function("bench.*", &java_from, "*/*")
                            .await
                            .expect("import");
                    }
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_native_sql,
    bench_ffi_scalar,
    bench_ffi_aggregate,
    bench_python_scalar,
    bench_go_scalar,
    bench_java_scalar,
    bench_function_import,
    bench_http_connector,
    bench_http_connector_query,
);

bench_helpers::bench_main!(benches);
