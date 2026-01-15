/// Generic observability instrumentation using OpenTelemetry
///
/// This module provides observability for all operations in Bundlebase including:
/// - Distributed tracing with spans
/// - Operation outcomes (success, miss, error, fallback, skipped)
/// - Operation latency
/// - Cache hit rates
/// - Bytes processed
///
/// Metrics and traces are exported via OpenTelemetry and can be consumed by Prometheus,
/// Jaeger, or any OTel-compatible backend.
use opentelemetry::{
    global::BoxedSpan,
    metrics::{Counter, Histogram, Meter, ObservableGauge},
    trace::{Span as OtelSpanTrait, Status, Tracer},
};


use lazy_static::lazy_static;

// Re-export KeyValue for use by callers
pub use opentelemetry::KeyValue;

mod logging;
pub use logging::{init_logging_metrics, init_logging_metrics_with_interval, log_current_metrics};

// Progress tracking integration (when metrics feature enabled)
pub mod progress;

pub use progress::{CompositeTracker, SpanProgressTracker};

/// Generic outcome for all operations
#[derive(Debug, Clone, Copy)]
pub enum OperationOutcome {
    /// Operation completed successfully
    Success,
    /// Resource not found (cache miss, index miss, etc.)
    Miss,
    /// Operation failed
    Error,
    /// Fell back to alternative approach
    Fallback,
    /// Operation skipped (e.g., selectivity check)
    Skipped,
}

impl OperationOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationOutcome::Success => "success",
            OperationOutcome::Miss => "miss",
            OperationOutcome::Error => "error",
            OperationOutcome::Fallback => "fallback",
            OperationOutcome::Skipped => "skipped",
        }
    }
}

/// Categories of operations for metric labeling
#[derive(Debug, Clone, Copy)]
pub enum OperationCategory {
    /// Index operations (lookups, builds)
    Index,
    /// Query operations (filter, select, join, sql)
    Select,
    /// I/O operations (file reads/writes)
    IO,
    /// Cache operations
    Cache,
    /// Commit operations
    Commit,
    /// Attach operations
    Attach,
}

impl OperationCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            OperationCategory::Index => "index",
            OperationCategory::Select => "select",
            OperationCategory::IO => "io",
            OperationCategory::Cache => "cache",
            OperationCategory::Commit => "commit",
            OperationCategory::Attach => "attach",
        }
    }

    /// Map an operation name string to an OperationCategory
    ///
    /// This is used for progress tracking integration where operation names
    /// are strings like "Attaching 'file.csv'" or "Indexing column 'user_id'".
    pub fn from_operation_name(name: &str) -> Self {
        if name.starts_with("Attaching") {
            OperationCategory::Attach
        } else if name.starts_with("Indexing") {
            OperationCategory::Index
        } else if name.starts_with("Querying") || name.starts_with("Filtering") {
            OperationCategory::Select
        } else if name.starts_with("Committing") {
            OperationCategory::Commit
        } else {
            OperationCategory::IO // default
        }
    }
}

/// Distributed tracing span that automatically finishes when dropped (RAII pattern)
///
/// Spans represent units of work and can be nested to show parent-child relationships.
/// Use `start_span()` to create a new span, add attributes with `set_attribute()`,
/// record events with `add_event()`, and the span will automatically finish when dropped.
pub struct Span {
    inner: BoxedSpan,
}

impl Span {
    /// Start a new span
    pub fn start(category: OperationCategory, operation: impl Into<String>) -> Self {
        let operation = operation.into();

        {
            let mut span = TRACER.start(format!("{}.{}", category.as_str(), operation));
            span.set_attribute(KeyValue::new("category", category.as_str()));
            span.set_attribute(KeyValue::new("operation", operation.clone()));

            Self { inner: span }
        }
    }

    /// Add an attribute to the span
    pub fn set_attribute(&mut self, key: &str, value: impl Into<String>) {
        self.inner
            .set_attribute(KeyValue::new(key.to_string(), value.into()));
    }

    /// Record an event in the span
    pub fn add_event(&mut self, name: &str, attributes: Vec<KeyValue>) {
        self.inner.add_event(name.to_string(), attributes);
    }

    /// Set the span status based on operation outcome
    pub fn set_outcome(&mut self, outcome: OperationOutcome) {
        let status = match outcome {
            OperationOutcome::Success => Status::Ok,
            OperationOutcome::Error => Status::error("Operation failed"),
            OperationOutcome::Miss | OperationOutcome::Fallback | OperationOutcome::Skipped => {
                Status::Unset
            }
        };
        self.inner.set_status(status);
        self.inner
            .set_attribute(KeyValue::new("outcome", outcome.as_str()));
    }

    /// Record an error in the span
    pub fn record_error(&mut self, error: &str) {
        self.inner.set_status(Status::error(error.to_string()));
        self.inner.add_event(
            "exception",
            vec![KeyValue::new("exception.message", error.to_string())],
        );
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        // Span automatically ends when dropped (RAII pattern)
        self.inner.end();
    }
}

/// Start a new tracing span
pub fn start_span(category: OperationCategory, operation: impl Into<String>) -> Span {
    Span::start(category, operation)
}

lazy_static! {
    /// Global tracer for distributed tracing
    static ref TRACER: opentelemetry::global::BoxedTracer = {
        opentelemetry::global::tracer("bundlebase")
    };


    /// Global meter for bundlebase metrics
    static ref METER: Meter = {
        opentelemetry::global::meter("bundlebase")
    };

    /// Counter for operation attempts by category and outcome
    static ref OPERATIONS: Counter<u64> = METER
        .u64_counter("bundlebase.operations")
        .with_description("Number of operations by category and outcome")
        .with_unit("operations")
        .init();

    /// Histogram for operation latency
    static ref OPERATION_DURATION: Histogram<f64> = METER
        .f64_histogram("bundlebase.duration")
        .with_description("Duration of operations in milliseconds")
        .with_unit("ms")
        .init();

    /// Counter for bytes processed (read/written)
    static ref BYTES_PROCESSED: Counter<u64> = METER
        .u64_counter("bundlebase.bytes")
        .with_description("Total bytes processed by operations")
        .with_unit("bytes")
        .init();

    /// Counter for cache operations
    static ref CACHE_OPERATIONS: Counter<u64> = METER
        .u64_counter("bundlebase.cache.operations")
        .with_description("Cache hits and misses")
        .with_unit("operations")
        .init();

    /// Gauge for current cache size
    static ref CACHE_SIZE: ObservableGauge<u64> = METER
        .u64_observable_gauge("bundlebase.cache.size")
        .with_description("Current number of entries in cache")
        .with_unit("entries")
        .init();
}

/// Records an operation attempt with outcome and category
pub fn record_operation(
    category: OperationCategory,
    outcome: OperationOutcome,
    operation: &str,
    labels: &[KeyValue],
) {
    let mut attrs = vec![
        KeyValue::new("category", category.as_str()),
        KeyValue::new("outcome", outcome.as_str()),
        KeyValue::new("operation", operation.to_string()),
    ];
    attrs.extend_from_slice(labels);
    OPERATIONS.add(1, &attrs);
}

/// Records operation duration
pub fn record_duration(
    category: OperationCategory,
    duration_ms: f64,
    operation: &str,
    outcome: OperationOutcome,
    labels: &[KeyValue],
) {
    let mut attrs = vec![
        KeyValue::new("category", category.as_str()),
        KeyValue::new("operation", operation.to_string()),
        KeyValue::new("outcome", outcome.as_str()),
    ];
    attrs.extend_from_slice(labels);
    OPERATION_DURATION.record(duration_ms, &attrs);
}

/// Records bytes processed (read/written)
pub fn record_bytes(category: OperationCategory, bytes: u64, operation: &str, labels: &[KeyValue]) {
    let mut attrs = vec![
        KeyValue::new("category", category.as_str()),
        KeyValue::new("operation", operation.to_string()),
    ];
    attrs.extend_from_slice(labels);
    BYTES_PROCESSED.add(bytes, &attrs);
}

/// Records a cache operation
pub fn record_cache_operation(cache_name: &str, hit: bool) {
    CACHE_OPERATIONS.add(
        1,
        &[
            KeyValue::new("cache_name", cache_name.to_string()),
            KeyValue::new("result", if hit { "hit" } else { "miss" }),
        ],
    );
}

/// Updates the cache size gauge
pub fn update_cache_size(cache_name: &str, size: u64) {
    // For observable gauges, we need to register a callback
    // This is typically done once at initialization
    let _ = (cache_name, size); // Placeholder - actual implementation would use callback
}

/// Generic operation timer that records duration and outcome
pub struct OperationTimer {
    category: OperationCategory,
    operation: String,
    labels: Vec<KeyValue>,
    start: std::time::Instant,
}

impl OperationTimer {
    /// Start timing an operation
    pub fn start(category: OperationCategory, operation: impl Into<String>) -> Self {
        Self {
            category,
            operation: operation.into(),
            labels: Vec::new(),
            start: std::time::Instant::now(),
        }
    }

    /// Add a label to the timer
    pub fn with_label(mut self, key: &str, value: impl Into<String>) -> Self {
        self.labels
            .push(KeyValue::new(key.to_string(), value.into()));
        self
    }

    /// Finish timing and record the outcome
    pub fn finish(self, outcome: OperationOutcome) {
        let duration_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        record_duration(
            self.category,
            duration_ms,
            &self.operation,
            outcome,
            &self.labels,
        );

        record_operation(self.category, outcome, &self.operation, &self.labels);
    }
}

/// Initialize progress tracking with span integration
///
/// This sets up a CompositeTracker that forwards progress updates to both
/// the logging tracker and the span tracker, allowing progress information
/// to appear in distributed traces.
pub fn init_progress_with_spans() {
    use crate::progress::{set_tracker, LoggingTracker};
    use std::sync::Arc;

    let composite = CompositeTracker::new(vec![
        Arc::new(LoggingTracker::new()),
        Arc::new(SpanProgressTracker::new()),
    ]);

    set_tracker(Box::new(composite));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_metrics() {
        // These shouldn't panic
        record_operation(
            OperationCategory::Select,
            OperationOutcome::Success,
            "test_op",
            &[],
        );
        record_duration(
            OperationCategory::Select,
            10.5,
            "test_op",
            OperationOutcome::Success,
            &[],
        );
        record_bytes(OperationCategory::IO, 1024, "test_op", &[]);
        record_cache_operation("test_cache", true);
    }

    #[test]
    fn test_timer() {
        let timer = OperationTimer::start(OperationCategory::Select, "test_op")
            .with_label("test_label", "test_value");
        std::thread::sleep(std::time::Duration::from_millis(1));
        timer.finish(OperationOutcome::Success);
    }
}
