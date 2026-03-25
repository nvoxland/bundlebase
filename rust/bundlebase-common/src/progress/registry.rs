//! Global progress tracker registry.
//!
//! Provides thread-safe storage and access to the active progress tracker.

use super::{LoggingTracker, ProgressTracker};
use lazy_static::lazy_static;
use parking_lot::RwLock;
use std::sync::Arc;

lazy_static! {
/// Global progress tracker instance.
///
/// By default, this holds a NoOpTracker. Applications can replace it with
/// their own implementation using `set_tracker()`.
    static ref PROGRESS_TRACKER: RwLock<Arc<dyn ProgressTracker + Send + Sync>> =
        RwLock::new(Arc::new(LoggingTracker::new()));
}

/// Get a clone of the current progress tracker.
///
/// Returns an Arc to the tracker, which is cheap to clone and can be held
/// across async boundaries.
///
/// # Returns
///
/// Arc to the currently registered tracker (default: NoOpTracker)
///
/// # Example
///
/// ```rust,ignore
/// let tracker = get_tracker();
/// let id = tracker.start("My operation", Some(100));
/// tracker.finish(id);
/// ```
pub fn get_tracker() -> Arc<dyn ProgressTracker + Send + Sync> {
    PROGRESS_TRACKER.read().clone()
}

/// Replace the global progress tracker.
///
/// This function is typically called once during application initialization
/// to install a custom tracker (e.g., tqdm in Python, indicatif in REPL).
///
/// # Arguments
///
/// * `tracker` - The new tracker to install
///
/// # Example
///
/// ```rust,ignore
/// use bundlebase::progress::{set_tracker, ProgressTracker, ProgressId};
///
/// struct MyTracker;
///
/// impl ProgressTracker for MyTracker {
///     fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
///         println!("Starting: {}", operation);
///         ProgressId::new()
///     }
///     fn update(&self, _id: ProgressId, current: u64, _message: Option<&str>) {
///         println!("Progress: {}", current);
///     }
///     fn finish(&self, _id: ProgressId) {
///         println!("Finished!");
///     }
/// }
///
/// set_tracker(Box::new(MyTracker));
/// ```
pub fn set_tracker(tracker: Box<dyn ProgressTracker + Send + Sync>) {
    let mut global = PROGRESS_TRACKER.write();
    *global = Arc::from(tracker);
}

/// Temporarily replace the tracker for the duration of a function.
///
/// Useful for testing - allows tests to install a mock tracker, run code,
/// and automatically restore the previous tracker.
///
/// # Arguments
///
/// * `tracker` - Temporary tracker to install
/// * `f` - Function to run with the temporary tracker
///
/// # Returns
///
/// The return value of `f`
///
/// # Example
///
/// ```rust,ignore
/// use bundlebase::progress::{with_tracker, MockTracker, ProgressScope};
///
/// let mock = MockTracker::new();
/// with_tracker(Box::new(mock.clone()), || {
///     let _scope = ProgressScope::new("Test", Some(10));
///     // Verify mock received calls
///     assert_eq!(mock.calls().len(), 1);
/// });
/// // Previous tracker restored here
/// ```
pub fn with_tracker<F, R>(tracker: Box<dyn ProgressTracker + Send + Sync>, f: F) -> R
where
    F: FnOnce() -> R,
{
    // Save current tracker
    let previous = {
        let mut global = PROGRESS_TRACKER.write();
        let previous = global.clone();
        *global = Arc::from(tracker);
        previous
    };

    // Run function with new tracker
    let result = f();

    // Restore previous tracker
    {
        let mut global = PROGRESS_TRACKER.write();
        *global = previous;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::ProgressId;
    use serial_test::serial;

    struct CountingTracker {
        count: Arc<parking_lot::Mutex<usize>>,
    }

    impl CountingTracker {
        fn new() -> Self {
            Self {
                count: Arc::new(parking_lot::Mutex::new(0)),
            }
        }
    }

    impl ProgressTracker for CountingTracker {
        fn start(&self, _operation: &str, _total: Option<u64>) -> ProgressId {
            *self.count.lock() += 1;
            ProgressId::new()
        }

        fn update(&self, _id: ProgressId, _current: u64, _message: Option<&str>) {
            *self.count.lock() += 1;
        }

        fn finish(&self, _id: ProgressId) {
            *self.count.lock() += 1;
        }
    }

    #[test]
    #[serial]
    fn test_default_tracker_is_noop() {
        let tracker = get_tracker();
        let id = tracker.start("Test", None);
        tracker.finish(id);
        // Should not panic
    }

    #[test]
    #[serial]
    fn test_set_tracker() {
        let counter = CountingTracker::new();
        let count_ref = counter.count.clone();

        set_tracker(Box::new(counter));

        let tracker = get_tracker();
        let id = tracker.start("Test", Some(10));
        tracker.update(id, 5, None);
        tracker.finish(id);

        // Should have counted 3 calls
        assert_eq!(*count_ref.lock(), 3);

        // Reset to NoOp for other tests
        set_tracker(Box::new(LoggingTracker::new()));
    }

    #[test]
    #[serial]
    fn test_with_tracker_restores_previous() {
        // Set a counter tracker globally
        let global_counter = CountingTracker::new();
        let global_count = global_counter.count.clone();
        set_tracker(Box::new(global_counter));

        // Use a temporary tracker
        let temp_counter = CountingTracker::new();
        let temp_count = temp_counter.count.clone();

        with_tracker(Box::new(temp_counter), || {
            let tracker = get_tracker();
            let id = tracker.start("Temp", None);
            tracker.finish(id);
        });

        // Temp tracker should have been used
        assert_eq!(*temp_count.lock(), 2);

        // Global tracker should not have been called during with_tracker
        assert_eq!(*global_count.lock(), 0);

        // Global tracker should be restored
        let tracker = get_tracker();
        let id = tracker.start("Global", None);
        tracker.finish(id);
        assert_eq!(*global_count.lock(), 2);

        // Reset to NoOp for other tests
        set_tracker(Box::new(LoggingTracker::new()));
    }

    #[test]
    #[serial]
    fn test_with_tracker_returns_value() {
        let result = with_tracker(Box::new(LoggingTracker::new()), || 42);
        assert_eq!(result, 42);
    }
}
