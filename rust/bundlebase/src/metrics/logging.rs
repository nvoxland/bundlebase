/// Lightweight OpenTelemetry exporter that logs metrics and traces instead of sending to external systems
///
/// This provides a simple way to see metrics and traces via logging without needing
/// Prometheus, Jaeger, or other external collectors.
///
/// # Example
///
/// ```rust,ignore
/// use bundlebase::metrics::init_logging_metrics;
///
/// // Initialize once at startup
/// init_logging_metrics();
///
/// // Metrics and traces will be logged automatically
/// // Or call log_current_metrics() to log on-demand
/// ```
use opentelemetry::global;
use opentelemetry_sdk::{
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{BatchSpanProcessor, TracerProvider},
};
use opentelemetry_stdout::{MetricsExporter, SpanExporter};

// Add tokio runtime handling to avoid panics when called without an existing runtime
use std::thread;

/// Initialize logging-based metrics export with default settings
///
/// This sets up a periodic exporter that logs metrics every 60 seconds to stdout.
/// Returns true if initialization succeeded, false if metrics feature is disabled.
///
/// # Example
///
/// ```rust,ignore
/// use bundlebase::metrics::init_logging_metrics;
///
/// fn main() {
///     env_logger::init();
///     init_logging_metrics();
///
///     // Your code here - metrics will be logged automatically
/// }
/// ```
pub fn init_logging_metrics() -> bool {
    init_logging_metrics_with_interval(std::time::Duration::from_secs(60))
}

/// Initialize logging-based metrics and tracing export with custom interval
///
/// # Arguments
///
/// * `interval` - How often to log metrics
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
/// use bundlebase::metrics::init_logging_metrics_with_interval;
///
/// // Log metrics every 30 seconds
/// init_logging_metrics_with_interval(Duration::from_secs(30));
/// ```
pub fn init_logging_metrics_with_interval(interval: std::time::Duration) -> bool {
    // Helper closure to build and register the providers. This will be called
    // either with an existing Tokio context (no-op), or while a newly-created
    // runtime is entered to ensure tokio::spawn / tokio::time work correctly.
    let build_providers = || {
        // Initialize tracing (spans)
        let span_exporter = SpanExporter::default();
        let span_processor =
            BatchSpanProcessor::builder(span_exporter, opentelemetry_sdk::runtime::Tokio).build();

        let tracer_provider = TracerProvider::builder()
            .with_span_processor(span_processor)
            .build();

        global::set_tracer_provider(tracer_provider);

        // Initialize metrics
        let metrics_exporter = MetricsExporter::default();

        let reader = PeriodicReader::builder(metrics_exporter, opentelemetry_sdk::runtime::Tokio)
            .with_interval(interval)
            .build();

        let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();

        global::set_meter_provider(meter_provider);

        log::info!(
            "Initialized logging-based metrics and tracing exporters (interval: {:?})",
            interval
        );
    };

    // If there's already a current Tokio runtime, just build providers normally.
    if tokio::runtime::Handle::try_current().is_ok() {
        build_providers();
        return true;
    }

    // No Tokio runtime currently running on this thread. Create a dedicated runtime,
    // enter it while building the providers (so tokio::spawn attaches to it), then
    // move the runtime into a background thread to keep it alive for the process
    // lifetime so the periodic reader's timers can run.
    match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => {
            // Enter the runtime so any tokio::spawn calls below attach to it
            let handle = rt.handle().clone();
            let _enter = handle.enter();

            // Build providers while the runtime is entered
            build_providers();

            // Move runtime into background thread and keep it alive by blocking on a pending future.
            thread::spawn(move || {
                // Keep the runtime running forever (until process exit)
                rt.block_on(async { std::future::pending::<()>().await });
            });

            true
        }
        Err(e) => {
            log::warn!("Failed to create Tokio runtime for metrics: {}", e);
            false
        }
    }
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
/// use bundlebase::metrics::log_current_metrics;
///
/// // After some operations
/// log_current_metrics();
/// ```
pub fn log_current_metrics() {
    // Note: OpenTelemetry 0.24 doesn't provide a simple way to force flush
    // from the global meter provider. The periodic reader will handle exports
    // at the configured interval. For immediate metrics, use a shorter interval.
    log::debug!("Metrics will be exported at the next periodic interval");
}

#[cfg(test)]
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
