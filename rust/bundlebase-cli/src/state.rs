//! Shared state for the bundlebase CLI.
//!
//! This module provides the `BundleState` type that wraps a `BundleBuilder`
//! with thread-safe access for use across different CLI modes (REPL, Flight, etc.).

use bundlebase::BundleBuilder;
use parking_lot::RwLock;

/// Shared state containing the bundle being worked on.
///
/// This type is designed to be wrapped in an `Arc` and shared across
/// async tasks and different CLI components.
pub struct BundleState {
    /// The bundle builder with read-write lock for thread-safe access.
    pub bundle: RwLock<BundleBuilder>,
}

impl BundleState {
    /// Create a new state wrapping a bundle builder.
    pub fn new(bundle: BundleBuilder) -> Self {
        Self {
            bundle: RwLock::new(bundle),
        }
    }
}

// Type alias for backwards compatibility
#[deprecated(since = "0.4.0", note = "Use BundleState instead")]
pub type State = BundleState;
