#![deny(clippy::unwrap_used)]
extern crate core;

// Internal aliases for bundlebase-common modules (crate-private)
pub(crate) use bundlebase_common::*;
pub use bundlebase_common::BundlebaseError;
pub use bundlebase_common::impl_dyn_command_response;
pub(crate) use bundlebase_common::arrow_types;
pub(crate) use bundlebase_common::namespaced_name;
pub(crate) use bundlebase_common::object_id;
pub(crate) use bundlebase_common::platform;
pub(crate) use bundlebase_common::progress;
pub(crate) use bundlebase_common::versioning;

pub mod bundle;
pub mod bundle_config;
pub mod catalog;
mod data;

mod index;
pub mod metrics;

// Internal alias for bundlebase-io (crate-private)
pub(crate) use bundlebase_io as io;
pub mod source;
#[allow(clippy::unwrap_used)]
pub mod test_utils;
// Re-export bundlebase-udf as internal modules for backward compatibility
pub mod function {
    pub use bundlebase_udf::bridge::ipc_bridge;
    pub use bundlebase_udf::bridge::ffi_bridge;
    pub use bundlebase_udf::bridge::manifest;
    pub use bundlebase_udf::bridge::python_bridge;
    pub use bundlebase_udf::bridge::scalar;
    pub use bundlebase_udf::bridge::aggregate;
    pub use bundlebase_udf::bridge::version_function as bundle_info;
    pub use bundlebase_udf::parse_python_entrypoint;
    pub use bundlebase_udf::VersionFunction;
}
pub(crate) mod udf {
    pub use bundlebase_udf::runtime::{UdfRuntime, RuntimeType};
}
#[allow(hidden_glob_reexports)]
pub(crate) mod connector;

pub use crate::bundle::{
    AnyOperation, Bundle, BundleBuilder, BundleChange, BundleCommit, BundleFacade,
    FileVerificationResult, Operation, ReportEntry, VerificationResults,
    META_DIR, INIT_FILENAME, HollowContext, ExpectedColumn,
};
pub use crate::bundle_config::{BundleConfig, ConfigScope, ConfigValueDetails, ConfigSource, PassedBundleConfig, Scope};
pub use bundle::JoinTypeOption;
pub use catalog::{CATALOG_NAME, BUNDLE_INFO_SCHEMA, DEFAULT_SCHEMA, tables as catalog_tables};
