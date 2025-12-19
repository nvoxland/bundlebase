//! Mock progress tracker for testing.
//!
//! This module provides a MockTracker that records all progress calls,
//! allowing tests to verify that progress tracking is working correctly.

use super::{ProgressId, ProgressTracker};
use parking_lot::Mutex;
use std::panic::RefUnwindSafe;
use std::sync::Arc;

/// A progress call recorded by the mock tracker.
#[derive(Debug, Clone, PartialEq)]
pub enum ProgressCall {
    /// A start() call was made.
    Start {
        id: ProgressId,
        operation: String,
        total: Option<u64>,
    },
    /// An update() call was made.
    Update {
        id: ProgressId,
        current: u64,
        message: Option<String>,
    },
    /// A finish() call was made.
    Finish { id: ProgressId },
}

/// Mock progress tracker that records all calls for verification in tests.
///
/// This tracker stores all progress calls in a Vec, allowing tests to assert
/// that operations are correctly reporting progress.
///
/// # Example
///
/// ```rust,ignore
/// use bundlebase::progress::{MockTracker, ProgressScope, with_tracker};
///
/// let mock = MockTracker::new();
/// with_tracker(Box::new(mock.clone()), || {
///     let _scope = ProgressScope::new("Test op", Some(100));
///
///     // Verify start was called
///     let calls = mock.calls();
///     assert_eq!(calls.len(), 1);
///     match &calls[0] {
///         ProgressCall::Start { operation, total, .. } => {
///             assert_eq!(operation, "Test op");
///             assert_eq!(*total, Some(100));
///         }
///         _ => panic!("Expected Start call"),
///     }
/// });
/// ```
#[derive(Clone)]
pub struct MockTracker {
    calls: Arc<Mutex<Vec<ProgressCall>>>,
}

impl MockTracker {
    /// Create a new mock tracker.
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get a snapshot of all recorded calls.
    ///
    /// Returns a clone of the call history. The mock continues to record
    /// calls after this method returns.
    pub fn calls(&self) -> Vec<ProgressCall> {
        self.calls.lock().clone()
    }

    /// Clear all recorded calls.
    ///
    /// Useful for resetting state between test assertions.
    pub fn clear(&self) {
        self.calls.lock().clear();
    }

    /// Get the number of calls recorded.
    pub fn call_count(&self) -> usize {
        self.calls.lock().len()
    }

    /// Find all Start calls in the history.
    pub fn starts(&self) -> Vec<ProgressCall> {
        self.calls()
            .into_iter()
            .filter(|call| matches!(call, ProgressCall::Start { .. }))
            .collect()
    }

    /// Find all Update calls in the history.
    pub fn updates(&self) -> Vec<ProgressCall> {
        self.calls()
            .into_iter()
            .filter(|call| matches!(call, ProgressCall::Update { .. }))
            .collect()
    }

    /// Find all Finish calls in the history.
    pub fn finishes(&self) -> Vec<ProgressCall> {
        self.calls()
            .into_iter()
            .filter(|call| matches!(call, ProgressCall::Finish { .. }))
            .collect()
    }
}

impl Default for MockTracker {
    fn default() -> Self {
        Self::new()
    }
}

// Implement RefUnwindSafe to allow MockTracker to be used in catch_unwind tests
impl RefUnwindSafe for MockTracker {}

impl ProgressTracker for MockTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        let id = ProgressId::new();
        self.calls.lock().push(ProgressCall::Start {
            id,
            operation: operation.to_string(),
            total,
        });
        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        self.calls.lock().push(ProgressCall::Update {
            id,
            current,
            message: message.map(|s| s.to_string()),
        });
    }

    fn finish(&self, id: ProgressId) {
        self.calls.lock().push(ProgressCall::Finish { id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_tracker_records_calls() {
        let mock = MockTracker::new();

        let id1 = mock.start("Op1", Some(100));
        mock.update(id1, 50, Some("Half"));
        mock.update(id1, 100, None);
        mock.finish(id1);

        let calls = mock.calls();
        assert_eq!(calls.len(), 4);

        match &calls[0] {
            ProgressCall::Start {
                operation, total, ..
            } => {
                assert_eq!(operation, "Op1");
                assert_eq!(*total, Some(100));
            }
            _ => panic!("Expected Start"),
        }

        match &calls[1] {
            ProgressCall::Update {
                current, message, ..
            } => {
                assert_eq!(*current, 50);
                assert_eq!(message.as_deref(), Some("Half"));
            }
            _ => panic!("Expected Update"),
        }

        match &calls[2] {
            ProgressCall::Update {
                current, message, ..
            } => {
                assert_eq!(*current, 100);
                assert_eq!(*message, None);
            }
            _ => panic!("Expected Update"),
        }

        match &calls[3] {
            ProgressCall::Finish { .. } => {}
            _ => panic!("Expected Finish"),
        }
    }

    #[test]
    fn test_mock_tracker_clear() {
        let mock = MockTracker::new();

        let id = mock.start("Test", None);
        mock.finish(id);

        assert_eq!(mock.call_count(), 2);

        mock.clear();
        assert_eq!(mock.call_count(), 0);
        assert_eq!(mock.calls().len(), 0);
    }

    #[test]
    fn test_mock_tracker_filter_methods() {
        let mock = MockTracker::new();

        let id1 = mock.start("Op1", Some(10));
        mock.update(id1, 5, None);
        let id2 = mock.start("Op2", None);
        mock.update(id2, 3, None);
        mock.finish(id1);
        mock.finish(id2);

        assert_eq!(mock.starts().len(), 2);
        assert_eq!(mock.updates().len(), 2);
        assert_eq!(mock.finishes().len(), 2);
    }

    #[test]
    fn test_mock_tracker_multiple_operations() {
        let mock = MockTracker::new();

        let id1 = mock.start("Operation A", Some(100));
        let id2 = mock.start("Operation B", None);

        mock.update(id1, 50, None);
        mock.update(id2, 25, Some("Status"));

        mock.finish(id1);
        mock.finish(id2);

        let calls = mock.calls();
        assert_eq!(calls.len(), 6); // 2 starts + 2 updates + 2 finishes
    }

    #[test]
    fn test_mock_tracker_clone_shares_state() {
        let mock1 = MockTracker::new();
        let mock2 = mock1.clone();

        let id = mock1.start("Test", None);
        mock2.finish(id);

        // Both should see the same calls
        assert_eq!(mock1.call_count(), 2);
        assert_eq!(mock2.call_count(), 2);
    }
}
