//! Cached benchmark data files
//!
//! Generates data files on first use and caches them to `benches/data/`.
//! Returns file:// URLs that can be used directly with `attach()`.

use arrow::array::RecordBatch;
use crate::data_generator::{generate_batch, generate_lookup_batch, BenchmarkDataConfig};
use parquet::arrow::ArrowWriter;
use std::path::{Path, PathBuf};
use url::Url;

/// Supported output formats for benchmark data.
#[derive(Copy, Clone)]
pub enum Format {
    Parquet,
    Csv,
    Json,
}

/// All supported formats for iteration in benchmarks.
pub const ALL_FORMATS: [Format; 3] = [Format::Parquet, Format::Csv, Format::Json];

impl Format {
    /// File extension and human-readable name for this format.
    pub fn name(&self) -> &str {
        match self {
            Format::Parquet => "parquet",
            Format::Csv => "csv",
            Format::Json => "json",
        }
    }
}

/// Directory where cached data files are stored.
fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/data")
}

/// Serialize a RecordBatch to the given format.
fn write_batch_to_format(batch: &RecordBatch, format: &Format) -> Vec<u8> {
    let mut buffer = Vec::new();
    match format {
        Format::Parquet => {
            let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), None)
                .expect("failed to create parquet writer");
            writer.write(batch).expect("failed to write parquet batch");
            writer.close().expect("failed to close parquet writer");
        }
        Format::Csv => {
            let mut writer = arrow::csv::WriterBuilder::new()
                .with_header(true)
                .build(&mut buffer);
            writer.write(batch).expect("failed to write csv batch");
        }
        Format::Json => {
            let mut writer = arrow::json::LineDelimitedWriter::new(&mut buffer);
            writer.write(batch).expect("failed to write json batch");
            writer.finish().expect("failed to finish json writer");
        }
    }
    buffer
}

/// Ensure a cached file exists at `path`, generating it with `generate` if missing.
fn ensure_cached(path: &Path, generate: impl FnOnce() -> Vec<u8>) -> String {
    let dir = path.parent().expect("path has parent");
    std::fs::create_dir_all(dir).expect("failed to create bench data dir");

    if !path.exists() {
        let buffer = generate();
        std::fs::write(path, &buffer).expect("failed to write cached file");
    }

    Url::from_file_path(path)
        .expect("valid file path")
        .to_string()
}

/// Return a file:// URL to cached data file, generating on first call.
pub fn get_data_url(rows: usize, format: &Format) -> String {
    let path = data_dir().join(format!("data_{}.{}", rows, format.name()));
    ensure_cached(&path, || {
        let config = BenchmarkDataConfig::with_rows(rows);
        let batch = generate_batch(&config);
        write_batch_to_format(&batch, format)
    })
}

/// Return a file:// URL to cached lookup table, generating on first call.
pub fn get_lookup_url(rows: usize, format: &Format) -> String {
    let path = data_dir().join(format!("lookup_{}.{}", rows, format.name()));
    ensure_cached(&path, || {
        let batch = generate_lookup_batch(rows);
        write_batch_to_format(&batch, format)
    })
}
