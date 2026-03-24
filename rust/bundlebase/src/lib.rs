#![deny(clippy::unwrap_used)]
extern crate core;

// Re-export everything from bundlebase-common so downstream crates
// can continue to use `bundlebase::ObjectId`, `bundlebase::ProgressTracker`, etc.
pub use bundlebase_common::*;

// Re-export common modules under their original names for internal `crate::` usage
pub use bundlebase_common::arrow_types;
pub use bundlebase_common::namespaced_name;
pub use bundlebase_common::object_id;
pub use bundlebase_common::platform;
pub use bundlebase_common::progress;
pub use bundlebase_common::row_id;
pub use bundlebase_common::versioning;

pub mod bundle;
pub mod bundle_config;
mod catalog;
mod data;

mod index;
pub mod metrics;

// Re-export bundlebase-io as the `io` module
pub use bundlebase_io as io;
pub mod source;
#[allow(clippy::unwrap_used)]
pub mod test_utils;
pub mod function;
#[allow(hidden_glob_reexports)]
pub(crate) mod connector;
pub(crate) mod udf;

pub use crate::bundle::{
    AnyOperation, Bundle, BundleBuilder, BundleChange, BundleCommit, BundleFacade,
    FileVerificationResult, Operation, VerificationResults,
};
pub use crate::bundle_config::{BundleConfig, ConfigScope, ConfigValueDetails, ConfigSource, PassedBundleConfig, Scope};
pub use crate::index::{IndexType, IndexTypeConfigError, ParseIndexTypeError, TokenizerConfig};
pub use bundle::JoinTypeOption;
pub use catalog::{CATALOG_NAME, BUNDLE_INFO_SCHEMA, DEFAULT_SCHEMA, tables as catalog_tables};
