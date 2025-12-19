//! Logging progress tracker implementation.
//!
//! This module provides a progress tracker that outputs progress events to the
//! Rust logging system using the `log` crate. Useful for server applications,
//! background jobs, and debugging.
//!
//! # Configuration
//!
//! The tracker respects the standard `RUST_LOG` environment variable:
//!
//! ```bash
//! RUST_LOG=bundlebase=debug cargo run # Debug and above
//! RUST_LOG=trace cargo run               # All events
//! ```

use super::{ProgressId, ProgressTracker};

/// Progress tracker that outputs to the Rust logging system.
///
/// This tracker logs progress events at debug level
///
/// # Example
///
/// ```rust,ignore
/// use bundlebase::progress::{LoggingTracker, set_tracker};
///
/// // Enable logging (requires env_logger or similar to be initialized)
/// env_logger::init();
///
/// // Register the tracker
/// set_tracker(Box::new(LoggingTracker::new()));
///
/// // Now all progress updates will be logged
/// let tracker = get_tracker();
/// let id = tracker.start("Processing data", Some(1000));
/// for i in 0..1000 {
///     // Work...
///     tracker.update(id, (i + 1) as u64, None);
/// }
/// tracker.finish(id);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct LoggingTracker;

impl LoggingTracker {
    /// Create a new logging tracker.
    pub fn new() -> Self {
        Self
    }
}

impl ProgressTracker for LoggingTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        let id = ProgressId::new();

        match total {
            Some(total) => {
                log::debug!(
                    "[Progress {}] Starting: {} (total: {})",
                    id,
                    operation,
                    total
                )
            }
            None => {
                log::debug!("[Progress {}] Starting: {} (indeterminate)", id, operation)
            }
        }

        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        match message {
            Some(msg) => {
                log::debug!("[Progress {}] Update: {} - {}", id, current, msg)
            }
            None => {
                log::debug!("[Progress {}] Update: {}", id, current)
            }
        }
    }

    fn finish(&self, id: ProgressId) {
        log::debug!("[Progress {}] Finished", id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logging_tracker_creation() {
        let tracker = LoggingTracker::new();
        let id = tracker.start("Test operation", Some(100));
        tracker.update(id, 50, None);
        tracker.finish(id);
        // Should not panic
    }

    #[test]
    fn test_logging_tracker_clone() {
        let tracker1 = LoggingTracker::new();
        let tracker2 = tracker1.clone();

        let id = tracker1.start("Test", Some(100));
        tracker1.finish(id);

        // Both trackers work independently (zero-sized)
        tracker2.finish(id);
    }

    #[test]
    fn test_logging_tracker_multiple_operations() {
        let tracker = LoggingTracker::new();

        let id1 = tracker.start("Op1", Some(100));
        let id2 = tracker.start("Op2", None);

        tracker.update(id1, 50, Some("Halfway"));
        tracker.update(id2, 25, None);

        tracker.finish(id1);
        tracker.finish(id2);
        // Should not panic
    }

    #[test]
    fn test_logging_tracker_indeterminate_progress() {
        let tracker = LoggingTracker::new();
        let id = tracker.start("Indeterminate", None);
        tracker.update(id, 1, Some("Step 1"));
        tracker.update(id, 2, Some("Step 2"));
        tracker.finish(id);
        // Should not panic
    }

    #[test]
    fn test_logging_tracker_with_special_characters() {
        let tracker = LoggingTracker::new();
        let id = tracker.start("Special: \"quotes\" & <brackets>", Some(100));
        tracker.update(id, 50, Some("Message with 🚀 emoji"));
        tracker.finish(id);
        // Should handle special characters gracefully
    }
}
