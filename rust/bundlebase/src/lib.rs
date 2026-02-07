#![deny(clippy::unwrap_used)]
extern crate core;

pub mod bundle;
pub mod bundle_config;
mod catalog;
mod data;
pub mod functions;
mod index;
pub mod io;
pub mod metrics;
pub mod object_id;
pub mod progress;
pub mod row_id;
pub mod source;
#[allow(clippy::unwrap_used)]
pub mod test_utils;
pub mod udf;
mod versioning;

pub use crate::bundle::{
    AnyOperation, Bundle, BundleBuilder, BundleChange, BundleCommit, BundleFacade,
    FileVerificationResult, Operation, VerificationResults,
};
pub use crate::bundle_config::{BundleConfig, ConfigScope, ConfigValueDetails, ConfigSource, PassedBundleConfig, Scope};
pub use crate::data::DataGenerator;
pub use crate::progress::{get_tracker, set_tracker, with_tracker, ProgressId, ProgressTracker};
pub use functions::{FunctionImpl, FunctionSignature};
pub use crate::index::{IndexType, IndexTypeConfigError, ParseIndexTypeError, TokenizerConfig};
use std::error::Error;
pub use bundle::JoinTypeOption;
pub use catalog::{CATALOG_NAME, BUNDLE_INFO_SCHEMA, DEFAULT_SCHEMA, tables as catalog_tables};

/// Standard error type used throughout the Bundlebase codebase
pub type BundlebaseError = Box<dyn Error + Send + Sync>;

/// All known configuration key specs for validation.
///
/// Thin wrapper around `BundleConfig::all_keys()` for backward compatibility.
/// TODO: remove
pub fn all_config_specs() -> Vec<bundle_config::ConfigKey> {
    BundleConfig::all_keys()
}

/// All known configuration scopes.
///
/// Thin wrapper around `BundleConfig::all_scopes()`.
/// TODO: remove
pub fn all_config_scopes() -> Vec<bundle_config::ConfigScope> {
    BundleConfig::all_scopes()
}

#[cfg(test)]
mod tests {
    // #[tokio::test]
    // fn it_works() {
    // let result = add(2, 2);
    // assert_eq!(result, 4);

    // query().await;
    // }
}
