//! Synthetic data generation for benchmarks
//!
//! Generates reproducible test data at runtime (not stored in repo).

#![allow(dead_code)]

use arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::Arc;

/// Configuration for benchmark data generation
#[derive(Clone)]
pub struct BenchmarkDataConfig {
    /// Number of rows to generate
    pub rows: usize,
    /// Number of columns to generate
    pub columns: usize,
    /// Random seed for reproducibility (default: 42)
    pub seed: u64,
}

impl Default for BenchmarkDataConfig {
    fn default() -> Self {
        Self {
            rows: 10_000,
            columns: 10,
            seed: 42,
        }
    }
}

impl BenchmarkDataConfig {
    /// Create config for specific row count
    pub fn with_rows(rows: usize) -> Self {
        Self {
            rows,
            ..Default::default()
        }
    }
}

/// Standard row counts for benchmarks
pub const SCALE_1K: usize = 1_000;
pub const SCALE_10K: usize = 10_000;
pub const SCALE_100K: usize = 100_000;
pub const SCALE_1M: usize = 1_000_000;
pub const SCALE_10M: usize = 10_000_000;

/// Generate a RecordBatch with synthetic data for benchmarking
///
/// Creates a mix of integer, float, and string columns with reproducible data.
pub fn generate_batch(config: &BenchmarkDataConfig) -> RecordBatch {
    let mut rng = StdRng::seed_from_u64(config.seed);

    // Generate integer IDs (sequential for index testing)
    let ids: Vec<i64> = (0..config.rows as i64).collect();
    let id_array = Int64Array::from(ids);

    // Generate random integers for filtering
    let filter_values: Vec<i64> = (0..config.rows).map(|_| rng.random_range(0..100)).collect();
    let filter_array = Int64Array::from(filter_values);

    // Generate random floats for aggregation
    let amounts: Vec<f64> = (0..config.rows)
        .map(|_| rng.random::<f64>() * 10000.0)
        .collect();
    let amount_array = Float64Array::from(amounts);

    // Generate category strings (limited set for grouping)
    let categories = ["A", "B", "C", "D", "E"];
    let category_values: Vec<&str> = (0..config.rows)
        .map(|_| categories[rng.random_range(0..categories.len())])
        .collect();
    let category_array = StringArray::from(category_values);

    // Generate random strings for text operations
    let names: Vec<String> = (0..config.rows)
        .map(|i| format!("item_{:08}", i))
        .collect();
    let name_array = StringArray::from(names.iter().map(|s| s.as_str()).collect::<Vec<_>>());

    // Generate secondary category for joins
    let regions = ["North", "South", "East", "West"];
    let region_values: Vec<&str> = (0..config.rows)
        .map(|_| regions[rng.random_range(0..regions.len())])
        .collect();
    let region_array = StringArray::from(region_values);

    // Build schema
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("filter_value", DataType::Int64, false),
        Field::new("amount", DataType::Float64, false),
        Field::new("category", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("region", DataType::Utf8, false),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_array),
            Arc::new(filter_array),
            Arc::new(amount_array),
            Arc::new(category_array),
            Arc::new(name_array),
            Arc::new(region_array),
        ],
    )
    .expect("Failed to create RecordBatch")
}

/// Generate a small lookup table for join benchmarks
pub fn generate_lookup_batch(rows: usize) -> RecordBatch {
    let mut rng = StdRng::seed_from_u64(42);

    let ids: Vec<i64> = (0..rows as i64).collect();
    let id_array = Int64Array::from(ids);

    let descriptions: Vec<String> = (0..rows)
        .map(|i| format!("description for item {}", i))
        .collect();
    let desc_array = StringArray::from(descriptions.iter().map(|s| s.as_str()).collect::<Vec<_>>());

    let multipliers: Vec<f64> = (0..rows).map(|_| rng.random::<f64>() * 2.0).collect();
    let mult_array = Float64Array::from(multipliers);

    let schema = Arc::new(Schema::new(vec![
        Field::new("lookup_id", DataType::Int64, false),
        Field::new("description", DataType::Utf8, false),
        Field::new("multiplier", DataType::Float64, false),
    ]));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_array),
            Arc::new(desc_array),
            Arc::new(mult_array),
        ],
    )
    .expect("Failed to create lookup RecordBatch")
}
