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
pub use crate::bundle_config::{BundleConfig, ConfigValueDetails, ConfigSource, PassedBundleConfig, Scope};
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
/// Each service defines its valid keys in its own module.
/// This function collects them for use by `BundleConfig::from_map()`.
pub fn all_config_specs() -> Vec<bundle_config::ConfigKey> {
    let mut specs = Vec::new();
    specs.extend_from_slice(io::plugin::S3_CONFIG_SPECS);
    specs.extend_from_slice(io::plugin::GCS_CONFIG_SPECS);
    specs.extend_from_slice(io::plugin::AZURE_CONFIG_SPECS);
    specs.extend_from_slice(io::plugin::SFTP_CONFIG_SPECS);
    specs.extend_from_slice(io::plugin::FTP_CONFIG_SPECS);
    specs.extend_from_slice(source::KAGGLE_CONFIG_SPECS);
    specs
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
