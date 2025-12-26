//! Progress tracking system for long-running operations.
//!
//! This module provides a pluggable progress tracking infrastructure that allows
//! bundlebase operations to report progress without being tightly coupled to
//! any specific UI implementation.
//!
//! # Architecture
//!
//! The system is built around three core concepts:
//!
//! 1. **ProgressTracker trait**: Defines the interface for receiving progress updates
//! 2. **Global registry**: Thread-safe storage for the active tracker
//! 3. **ProgressScope**: RAII wrapper ensuring cleanup via Drop
//!
//! # Default Behavior
//!
//! By default, a [`LoggingTracker`] is registered with debug level. For less overhead, there is a NoOp tracker available.
//! Operations always call progress tracking methods, but the no-op implementation
//! ensures no performance impact when progress tracking is not needed.
//!
//! # Usage
//!
//! ## In Operations
//!
//! ```rust,ignore
//! use bundlebase::progress::ProgressScope;
//!
//! async fn rebuild_index(&self) -> Result<(), Error> {
//!     let total_rows = self.estimate_rows().await?;
//!     let _scope = ProgressScope::new("Rebuilding index", total_rows);
//!
//!     let mut processed = 0;
//!     while let Some(batch) = stream.next().await {
//!         // Process batch...
//!         processed += batch.num_rows();
//!         _scope.update(processed, None);
//!     }
//!     // Drop automatically calls finish()
//!     Ok(())
//! }
//! ```
//!
//! ## Registering Custom Trackers
//!
//! ```rust,ignore
//! use bundlebase::progress::{ProgressTracker, set_tracker};
//!
//! struct MyTracker;
//!
//! impl ProgressTracker for MyTracker {
//!     fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
//!         println!("Starting: {} (total: {:?})", operation, total);
//!         ProgressId::new()
//!     }
//!
//!     fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
//!         println!("Progress {}: {}", id.0, current);
//!     }
//!
//!     fn finish(&self, id: ProgressId) {
//!         println!("Finished: {}", id.0);
//!     }
//! }
//!
//! set_tracker(Box::new(MyTracker));
//! ```

mod logging;
mod registry;

#[cfg(test)]
mod mock;

pub use logging::LoggingTracker;
pub use registry::{get_tracker, set_tracker, with_tracker};

use std::sync::atomic::{AtomicU64, Ordering};

/// Unique identifier for a progress operation.
///
/// Generated atomically to ensure uniqueness across concurrent operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgressId(pub u64);

impl ProgressId {
    /// Generate a new unique progress ID.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::SeqCst))
    }
}

impl Default for ProgressId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProgressId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Progress({})", self.0)
    }
}

/// Trait for receiving progress updates from long-running operations.
///
/// Implementations must be thread-safe (`Send + Sync`) as they will be called
/// from async tasks running on the tokio runtime.
///
/// # Thread Safety
///
/// All methods receive `&self` (not `&mut self`), so implementations must use
/// interior mutability (e.g., `Mutex`, `RwLock`) if they need to maintain state.
///
/// # Implementation Guidelines
///
/// - `start()` should return quickly and not block
/// - `update()` may be called frequently (e.g., per batch), so should be fast
/// - `finish()` should clean up any resources associated with the progress ID
/// - Implementations should handle unknown totals gracefully (None means indeterminate)
pub trait ProgressTracker: Send + Sync {
    /// Start tracking a new operation.
    ///
    /// # Arguments
    ///
    /// * `operation` - Human-readable description (e.g., "Rebuilding index on salary")
    /// * `total` - Expected total units of work, or None for indeterminate progress
    ///
    /// # Returns
    ///
    /// A unique identifier for this progress operation, used in subsequent updates.
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId;

    /// Update progress for an ongoing operation.
    ///
    /// # Arguments
    ///
    /// * `id` - The progress ID returned from `start()`
    /// * `current` - Current units of work completed
    /// * `message` - Optional status message (e.g., "Processing block 2/5")
    fn update(&self, id: ProgressId, current: u64, message: Option<&str>);

    /// Mark an operation as finished.
    ///
    /// Called automatically by `ProgressScope::drop()`, so implementations should
    /// ensure this is idempotent (safe to call multiple times).
    ///
    /// # Arguments
    ///
    /// * `id` - The progress ID to finish
    fn finish(&self, id: ProgressId);
}

/// RAII wrapper for progress tracking that ensures cleanup.
///
/// This struct ensures `finish()` is always called, even if the operation
/// panics or returns early. Create a scope at the start of an operation
/// and let it drop naturally at the end.
///
/// # Example
///
/// ```rust,ignore
/// async fn process_data(&self) -> Result<(), Error> {
///     let _scope = ProgressScope::new("Processing data", Some(1000));
///
///     for i in 0..1000 {
///         // Process item...
///         _scope.update(i as u64, None);
///
///         if some_error {
///             return Err(error); // finish() still called via Drop
///         }
///     }
///
///     Ok(())
/// } // finish() called automatically here
/// ```
pub struct ProgressScope {
    id: ProgressId,
    current: AtomicU64,
}

impl ProgressScope {
    /// Create a new progress scope and start tracking.
    ///
    /// # Arguments
    ///
    /// * `operation` - Description of the operation
    /// * `total` - Expected total, or None for indeterminate progress
    ///
    /// # Returns
    ///
    /// A scope that will automatically call `finish()` when dropped.
    pub fn new(operation: &str, total: Option<u64>) -> Self {
        let tracker = get_tracker();
        let id = tracker.start(operation, total);
        Self {
            id,
            current: AtomicU64::new(0),
        }
    }

    /// Update progress to a new current value.
    ///
    /// # Arguments
    ///
    /// * `current` - New current value
    /// * `message` - Optional status message
    pub fn update(&self, current: u64, message: Option<&str>) {
        self.current.store(current, Ordering::Relaxed);
        let tracker = get_tracker();
        tracker.update(self.id, current, message);
    }

    /// Increment progress by a delta.
    ///
    /// Useful for operations that process items one at a time.
    ///
    /// # Arguments
    ///
    /// * `delta` - Amount to increment by
    /// * `message` - Optional status message
    pub fn increment(&self, delta: u64, message: Option<&str>) {
        let new_current = self.current.fetch_add(delta, Ordering::Relaxed) + delta;
        let tracker = get_tracker();
        tracker.update(self.id, new_current, message);
    }

    /// Get the progress ID for this scope.
    pub fn id(&self) -> ProgressId {
        self.id
    }
}

impl Drop for ProgressScope {
    fn drop(&mut self) {
        let tracker = get_tracker();
        tracker.finish(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub use mock::{MockTracker, ProgressCall};
    use serial_test::serial;

    #[test]
    fn test_progress_id_uniqueness() {
        let id1 = ProgressId::new();
        let id2 = ProgressId::new();
        let id3 = ProgressId::new();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_progress_id_display() {
        let id = ProgressId(42);
        assert_eq!(format!("{}", id), "Progress(42)");
    }

    #[test]
    #[serial]
    fn test_progress_scope_lifecycle() {
        // Use mock tracker to verify calls
        let mock = MockTracker::new();
        with_tracker(Box::new(mock.clone()), || {
            {
                let _scope = ProgressScope::new("Test operation", Some(100));
                // start() called in constructor
            } // finish() called in drop

            let calls = mock.calls();
            assert_eq!(calls.len(), 2);

            match &calls[0] {
                ProgressCall::Start {
                    operation, total, ..
                } => {
                    assert_eq!(operation, "Test operation");
                    assert_eq!(*total, Some(100));
                }
                _ => panic!("Expected Start call"),
            }

            match &calls[1] {
                ProgressCall::Finish { .. } => {}
                _ => panic!("Expected Finish call"),
            }
        });
    }

    #[test]
    #[serial]
    fn test_progress_scope_update() {
        let mock = MockTracker::new();
        with_tracker(Box::new(mock.clone()), || {
            let scope = ProgressScope::new("Test", Some(10));
            scope.update(5, Some("Half done"));
            scope.update(10, None);

            let calls = mock.calls();
            assert_eq!(calls.len(), 3); // start, update, update (finish happens on drop)

            match &calls[1] {
                ProgressCall::Update {
                    current, message, ..
                } => {
                    assert_eq!(*current, 5);
                    assert_eq!(message.as_deref(), Some("Half done"));
                }
                _ => panic!("Expected Update call"),
            }

            match &calls[2] {
                ProgressCall::Update {
                    current, message, ..
                } => {
                    assert_eq!(*current, 10);
                    assert_eq!(*message, None);
                }
                _ => panic!("Expected Update call"),
            }
        });
    }

    #[test]
    #[serial]
    fn test_progress_scope_increment() {
        let mock = MockTracker::new();
        with_tracker(Box::new(mock.clone()), || {
            let scope = ProgressScope::new("Test", Some(10));
            scope.increment(3, None);
            scope.increment(2, None);
            scope.increment(5, None);

            let calls = mock.calls();
            // start + 3 increments (finish happens on drop later)
            assert!(calls.len() >= 4);

            // Verify incremental values
            match &calls[1] {
                ProgressCall::Update { current, .. } => assert_eq!(*current, 3),
                _ => panic!("Expected Update"),
            }
            match &calls[2] {
                ProgressCall::Update { current, .. } => assert_eq!(*current, 5),
                _ => panic!("Expected Update"),
            }
            match &calls[3] {
                ProgressCall::Update { current, .. } => assert_eq!(*current, 10),
                _ => panic!("Expected Update"),
            }
        });
    }
}
