//! Python bindings for progress tracking.
//!
//! This module provides the bridge between Rust progress tracking and Python
//! callbacks, allowing Python code to register custom progress trackers (e.g., tqdm).

use ::bundlebase::progress::{ProgressId, ProgressTracker};
use parking_lot::Mutex;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Python-based progress tracker that forwards calls to a Python callback.
///
/// This tracker wraps a Python callable and invokes it on each progress event.
/// The Python callback should have the signature:
/// `fn(operation: str, id: int, current: int, total: Optional[int], message: Optional[str])`
pub struct PyProgressTracker {
    /// Python callback function (thread-safe)
    callback: Py<PyAny>,
    /// Active progress operations (for tracking state if needed)
    operations: Arc<Mutex<HashMap<ProgressId, String>>>,
}

impl PyProgressTracker {
    /// Create a new Python progress tracker with the given callback.
    pub fn new(callback: Py<PyAny>) -> Self {
        Self {
            callback,
            operations: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ProgressTracker for PyProgressTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        let id = ProgressId::new();

        // Store operation name
        self.operations.lock().insert(id, operation.to_string());

        // Call Python callback with 'start' event
        let operation_str = operation.to_string();
        let _ = Python::attach(|py| {
            let callback = self.callback.clone_ref(py);
            let args = ("start", operation_str, id.0, 0u64, total, py.None());
            callback.call1(py, args)
        });

        id
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        let operation = self.operations.lock().get(&id).cloned();

        if let Some(op_name) = operation {
            let msg = message.map(|s| s.to_string());
            let _ = Python::attach(|py| {
                let callback = self.callback.clone_ref(py);
                let message_py = msg.as_ref().map(|s| s.as_str()).unwrap_or("");
                let args = (
                    "update",
                    op_name,
                    id.0,
                    current,
                    py.None(), // total not tracked in update
                    message_py,
                );
                callback.call1(py, args)
            });
        }
    }

    fn finish(&self, id: ProgressId) {
        let operation = self.operations.lock().remove(&id);

        if let Some(op_name) = operation {
            let _ = Python::attach(|py| {
                let callback = self.callback.clone_ref(py);
                let args = ("finish", op_name, id.0, 0u64, py.None(), py.None());
                callback.call1(py, args)
            });
        }
    }
}

/// Register a Python callback as the global progress tracker.
///
/// The callback will be called with events: 'start', 'update', and 'finish'.
///
/// # Arguments
///
/// * `callback` - Python callable with signature:
///   `fn(event: str, operation: str, id: int, current: int, total: Optional[int], message: Optional[str])`
///
/// # Example (Python)
///
/// ```python
/// def my_callback(event, operation, id, current, total, message):
///     if event == 'start':
///         print(f"Starting: {operation} (total: {total})")
///     elif event == 'update':
///         print(f"Progress: {current}/{total} - {message}")
///     elif event == 'finish':
///         print(f"Finished: {operation}")
///
/// bundlebase._register_progress_callback(my_callback)
/// ```
#[pyfunction]
fn _register_progress_callback(callback: Py<PyAny>) {
    let tracker = PyProgressTracker::new(callback);
    ::bundlebase::set_tracker(Box::new(tracker));
}

/// Python module initialization for progress tracking
pub fn register_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(_register_progress_callback, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase::progress::ProgressScope;

    #[test]
    fn test_py_progress_tracker_basic() {
        // Create a mock Python callback that does nothing
        Python::initialize();

        Python::attach(|py| {
            // Create a simple Python function
            let callback = py
                .eval(
                    c"lambda event, op, id, current, total, msg: None",
                    None,
                    None,
                )
                .unwrap();

            let tracker = PyProgressTracker::new(callback.into());

            // Test the tracker
            let id = tracker.start("Test operation", Some(100));
            tracker.update(id, 50, Some("Half done"));
            tracker.finish(id);
        });
    }

    #[test]
    fn test_register_callback() {
        Python::initialize();

        Python::attach(|py| {
            let callback = py
                .eval(
                    c"lambda event, op, id, current, total, msg: None",
                    None,
                    None,
                )
                .unwrap();

            _register_progress_callback(callback.into());

            // Create a scope to verify the callback is registered
            let _scope = ProgressScope::new("Test", Some(10));
        });
    }
}
