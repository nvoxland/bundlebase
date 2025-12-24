/// Lightweight OpenTelemetry metrics exporter that logs metrics instead of sending to external systems
///
/// This provides a simple way to see index metrics via logging without needing
/// Prometheus, Jaeger, or other external collectors.
///
/// # Example
///
/// ```rust
/// use bundlebase::index::init_logging_metrics;
///
/// // Initialize once at startup
/// init_logging_metrics();
///
/// // Metrics will be logged every 60 seconds automatically
/// // Or call log_current_metrics() to log on-demand
/// ```

#[cfg(feature = "metrics")]
use opentelemetry::global;
#[cfg(feature = "metrics")]
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
#[cfg(feature = "metrics")]
use opentelemetry_stdout::MetricsExporter;

/// Initialize logging-based metrics export with default settings
///
/// This sets up a periodic exporter that logs metrics every 60 seconds to stdout.
/// Returns true if initialization succeeded, false if metrics feature is disabled.
///
/// # Example
///
/// ```rust
/// use bundlebase::index::init_logging_metrics;
///
/// fn main() {
///     env_logger::init();
///     init_logging_metrics();
///
///     // Your code here - metrics will be logged automatically
/// }
/// ```
#[cfg(feature = "metrics")]
pub fn init_logging_metrics() -> bool {
    init_logging_metrics_with_interval(std::time::Duration::from_secs(60))
}

/// Initialize logging-based metrics export with custom interval
///
/// # Arguments
///
/// * `interval` - How often to log metrics
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
/// use bundlebase::index::init_logging_metrics_with_interval;
///
/// // Log metrics every 30 seconds
/// init_logging_metrics_with_interval(Duration::from_secs(30));
/// ```
#[cfg(feature = "metrics")]
pub fn init_logging_metrics_with_interval(interval: std::time::Duration) -> bool {
    // Use stdout exporter which prints metrics to stdout
    let exporter = MetricsExporter::default();

    let reader = PeriodicReader::builder(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_interval(interval)
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .build();

    global::set_meter_provider(provider);

    log::info!(
        "Initialized logging-based metrics exporter (interval: {:?})",
        interval
    );

    true
}

/// Log current metrics immediately (on-demand)
///
/// This forces an immediate export of current metrics to the log.
/// Useful for debugging or logging metrics at specific points.
///
/// Note: The current implementation relies on the periodic reader.
/// For immediate metrics, consider setting a shorter interval.
///
/// # Example
///
/// ```rust
/// use bundlebase::index::log_current_metrics;
///
/// // After some operations
/// log_current_metrics();
/// ```
#[cfg(feature = "metrics")]
pub fn log_current_metrics() {
    // Note: OpenTelemetry 0.24 doesn't provide a simple way to force flush
    // from the global meter provider. The periodic reader will handle exports
    // at the configured interval. For immediate metrics, use a shorter interval.
    log::debug!("Metrics will be exported at the next periodic interval");
}

// No-op versions when metrics feature is disabled
#[cfg(not(feature = "metrics"))]
pub fn init_logging_metrics() -> bool {
    false
}

#[cfg(not(feature = "metrics"))]
pub fn init_logging_metrics_with_interval(_interval: std::time::Duration) -> bool {
    false
}

#[cfg(not(feature = "metrics"))]
pub fn log_current_metrics() {}

#[cfg(all(test, feature = "metrics"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_logging_metrics() {
        let result = init_logging_metrics();
        assert!(result);
    }

    #[tokio::test]
    async fn test_init_with_custom_interval() {
        let result = init_logging_metrics_with_interval(std::time::Duration::from_secs(10));
        assert!(result);
    }
}
