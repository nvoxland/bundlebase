/// Integration between progress tracking and OpenTelemetry spans
///
/// This module provides progress trackers that feed updates into OpenTelemetry spans,
/// allowing progress information to appear in distributed traces.
use crate::metrics::{start_span, KeyValue, OperationCategory, OperationOutcome, Span};
use crate::progress::{ProgressId, ProgressTracker};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;


/// Progress tracker that feeds updates to OpenTelemetry spans
///
/// Creates a new span for each progress operation and updates it with progress events.
/// Progress updates are recorded as span events to avoid cardinality issues.
pub struct SpanProgressTracker {
    /// Stores active spans by ProgressId
    spans: Arc<RwLock<HashMap<ProgressId, Span>>>,
}

impl SpanProgressTracker {
    /// Create a new SpanProgressTracker
    pub fn new() -> Self {
        Self {
            spans: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for SpanProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressTracker for SpanProgressTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        let id = ProgressId::new();
        let category = OperationCategory::from_operation_name(operation);
        let mut span = start_span(category, operation);

        // Set progress metadata as span attributes
        if let Some(total) = total {
            span.set_attribute("progress.total", &total.to_string());
        }
        span.set_attribute("progress.determinate", &total.is_some().to_string());
        span.set_attribute("progress.operation", operation);

        self.spans.write().insert(id, span);
        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        if let Some(span) = self.spans.write().get_mut(&id) {
            // Add progress update as span event (not attribute - prevents cardinality explosion)
            let mut attrs = vec![KeyValue::new("progress.current", current.to_string())];
            if let Some(msg) = message {
                attrs.push(KeyValue::new("progress.message", msg.to_string()));
            }
            span.add_event("progress.update", attrs);
        }
    }

    fn finish(&self, id: ProgressId) {
        if let Some(mut span) = self.spans.write().remove(&id) {
            span.set_outcome(OperationOutcome::Success);
            // Span auto-finishes on drop
        }
    }
}

/// Composite tracker that forwards progress updates to multiple backends
///
/// Allows progress updates to feed multiple tracking systems simultaneously,
/// such as logging AND span tracking.
pub struct CompositeTracker {
    trackers: Vec<Arc<dyn ProgressTracker>>,
}

impl CompositeTracker {
    /// Create a new CompositeTracker from a list of trackers
    pub fn new(trackers: Vec<Arc<dyn ProgressTracker>>) -> Self {
        assert!(
            !trackers.is_empty(),
            "CompositeTracker requires at least one tracker"
        );
        Self { trackers }
    }
}

impl ProgressTracker for CompositeTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        // Use the first tracker's ID for all trackers
        let id = self.trackers[0].start(operation, total);

        // Start on remaining trackers (they won't use the returned ID, but need to track internally)
        for tracker in &self.trackers[1..] {
            tracker.start(operation, total);
        }

        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        for tracker in &self.trackers {
            tracker.update(id, current, message);
        }
    }

    fn finish(&self, id: ProgressId) {
        for tracker in &self.trackers {
            tracker.finish(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_progress_tracker_creation() {
        let tracker = SpanProgressTracker::new();
        let id = tracker.start("Test operation", Some(100));
        tracker.update(id, 50, Some("Halfway"));
        tracker.finish(id);
        // If this doesn't panic, the basic flow works
    }

    #[test]
    fn test_composite_tracker() {
        use crate::progress::LoggingTracker;

        let composite = CompositeTracker::new(vec![
            Arc::new(LoggingTracker::new()),
            Arc::new(SpanProgressTracker::new()),
        ]);

        let id = composite.start("Composite test", Some(10));
        composite.update(id, 5, Some("Middle"));
        composite.finish(id);
        // If this doesn't panic, composite tracking works
    }
}
