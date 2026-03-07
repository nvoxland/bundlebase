use crate::types::{Location, StableUrl};
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;

/// Trait for implementing a custom Bundlebase source function.
///
/// Implement [`discover`](Connector::discover) and [`data`](Connector::data).
/// Optionally override [`stable_url`](Connector::stable_url) for caching.
pub trait Connector {
    /// Return the available data locations.
    fn discover(
        &self,
        attached_locations: &[String],
        args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>>;

    /// Return Arrow record batches for the given location.
    /// Return `Ok(None)` for no data.
    fn data(
        &self,
        location: &Location,
        args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>>;

    /// Return a stable URL for the given location.
    /// Default implementation returns `Ok(None)`.
    fn stable_url(
        &self,
        _location: &Location,
        _args: &HashMap<String, String>,
    ) -> Result<Option<StableUrl>, Box<dyn std::error::Error>> {
        Ok(None)
    }
}

/// Backward-compatible alias for [`Connector`].
#[deprecated(note = "Use `Connector` instead")]
pub trait SourceFunction: Connector {}
