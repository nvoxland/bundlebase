/// Index metrics instrumentation using OpenTelemetry
///
/// This module provides observability for index operations including:
/// - Index hits/misses/errors
/// - Query latency
/// - Cache hit rates
/// - Index file sizes
///
/// Metrics are exported via OpenTelemetry and can be consumed by Prometheus,
/// Jaeger, or any OTel-compatible backend.

#[cfg(feature = "metrics")]
use opentelemetry::{
    metrics::{Counter, Histogram, Meter, ObservableGauge},
    KeyValue,
};

#[cfg(feature = "metrics")]
use lazy_static::lazy_static;

/// Index operation outcomes for tracking
#[derive(Debug, Clone, Copy)]
pub enum IndexOutcome {
    /// Index was used successfully
    Hit,
    /// No index available for column
    Miss,
    /// Index exists but couldn't be loaded/used
    Error,
    /// Query fell back to full scan
    Fallback,
}

impl IndexOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            IndexOutcome::Hit => "hit",
            IndexOutcome::Miss => "miss",
            IndexOutcome::Error => "error",
            IndexOutcome::Fallback => "fallback",
        }
    }
}

#[cfg(feature = "metrics")]
lazy_static! {
    /// Global meter for index metrics
    static ref METER: Meter = {
        opentelemetry::global::meter("bundlebase.index")
    };

    /// Counter for index lookup attempts by outcome
    static ref INDEX_LOOKUPS: Counter<u64> = METER
        .u64_counter("index.lookups")
        .with_description("Number of index lookup attempts by outcome")
        .with_unit("lookups")
        .init();

    /// Histogram for index lookup latency
    static ref INDEX_LOOKUP_DURATION: Histogram<f64> = METER
        .f64_histogram("index.lookup_duration")
        .with_description("Duration of index lookups in milliseconds")
        .with_unit("ms")
        .init();

    /// Counter for RowId cache operations
    static ref CACHE_OPERATIONS: Counter<u64> = METER
        .u64_counter("index.cache.operations")
        .with_description("RowId cache hits and misses")
        .with_unit("operations")
        .init();

    /// Gauge for current cache size
    static ref CACHE_SIZE: ObservableGauge<u64> = METER
        .u64_observable_gauge("index.cache.size")
        .with_description("Current number of entries in RowId cache")
        .with_unit("entries")
        .init();

    /// Counter for bytes loaded from index files
    static ref INDEX_BYTES_READ: Counter<u64> = METER
        .u64_counter("index.bytes_read")
        .with_description("Total bytes read from index files")
        .with_unit("bytes")
        .init();
}

/// Records an index lookup attempt
pub fn record_index_lookup(outcome: IndexOutcome, column: &str) {
    #[cfg(feature = "metrics")]
    {
        INDEX_LOOKUPS.add(
            1,
            &[
                KeyValue::new("outcome", outcome.as_str()),
                KeyValue::new("column", column.to_string()),
            ],
        );
    }

    #[cfg(not(feature = "metrics"))]
    {
        let _ = (outcome, column);
    }
}

/// Records index lookup latency
pub fn record_index_lookup_duration(duration_ms: f64, column: &str, outcome: IndexOutcome) {
    #[cfg(feature = "metrics")]
    {
        INDEX_LOOKUP_DURATION.record(
            duration_ms,
            &[
                KeyValue::new("column", column.to_string()),
                KeyValue::new("outcome", outcome.as_str()),
            ],
        );
    }

    #[cfg(not(feature = "metrics"))]
    {
        let _ = (duration_ms, column, outcome);
    }
}

/// Records a RowId cache operation
pub fn record_cache_operation(hit: bool) {
    #[cfg(feature = "metrics")]
    {
        CACHE_OPERATIONS.add(
            1,
            &[KeyValue::new("result", if hit { "hit" } else { "miss" })],
        );
    }

    #[cfg(not(feature = "metrics"))]
    {
        let _ = hit;
    }
}

/// Records bytes read from an index file
pub fn record_index_bytes_read(bytes: u64, column: &str) {
    #[cfg(feature = "metrics")]
    {
        INDEX_BYTES_READ.add(bytes, &[KeyValue::new("column", column.to_string())]);
    }

    #[cfg(not(feature = "metrics"))]
    {
        let _ = (bytes, column);
    }
}

/// Updates the cache size gauge
pub fn update_cache_size(size: u64) {
    #[cfg(feature = "metrics")]
    {
        // For observable gauges, we need to register a callback
        // This is typically done once at initialization
        let _ = size; // Placeholder - actual implementation would use callback
    }

    #[cfg(not(feature = "metrics"))]
    {
        let _ = size;
    }
}

/// Helper to measure execution time and record metrics
pub struct IndexLookupTimer {
    column: String,
    #[cfg(feature = "metrics")]
    start: std::time::Instant,
}

impl IndexLookupTimer {
    pub fn start(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            #[cfg(feature = "metrics")]
            start: std::time::Instant::now(),
        }
    }

    pub fn finish(self, outcome: IndexOutcome) {
        #[cfg(feature = "metrics")]
        {
            let duration_ms = self.start.elapsed().as_secs_f64() * 1000.0;
            record_index_lookup_duration(duration_ms, &self.column, outcome);
        }

        #[cfg(not(feature = "metrics"))]
        {
            let _ = outcome;
        }

        record_index_lookup(outcome, &self.column);
    }
}

#[cfg(all(test, feature = "metrics"))]
mod tests {
    use super::*;

    #[test]
    fn test_record_metrics() {
        // These shouldn't panic
        record_index_lookup(IndexOutcome::Hit, "test_column");
        record_index_lookup_duration(10.5, "test_column", IndexOutcome::Hit);
        record_cache_operation(true);
        record_index_bytes_read(1024, "test_column");
    }

    #[test]
    fn test_timer() {
        let timer = IndexLookupTimer::start("test_column");
        std::thread::sleep(std::time::Duration::from_millis(1));
        timer.finish(IndexOutcome::Hit);
    }
}

#[cfg(all(test, not(feature = "metrics")))]
mod tests {
    use super::*;

    #[test]
    fn test_no_op_metrics() {
        // When metrics feature is disabled, these should all be no-ops
        record_index_lookup(IndexOutcome::Hit, "test_column");
        record_index_lookup_duration(10.5, "test_column", IndexOutcome::Hit);
        record_cache_operation(true);
        record_index_bytes_read(1024, "test_column");

        let timer = IndexLookupTimer::start("test_column");
        timer.finish(IndexOutcome::Hit);
    }
}
