mod bundle_view_table;

pub use bundle_view_table::BundleViewTable;

use crate::bundle::BundleFacade;
use crate::BundlebaseError;
use datafusion::prelude::SessionContext;
use std::sync::{OnceLock, Weak};

/// Alias dataframe is registered in the ctx under. User can select from this
pub static BUNDLE_TABLE: &str = "bundle";

/// Datafusion catalog name used
pub static CATALOG_NAME: &str = "bundlebase";

/// Schema name for bundle metadata tables.
pub static BUNDLE_INFO_SCHEMA: &str = "bundle_info";

/// Schema name for the default data schema.
pub static DEFAULT_SCHEMA: &str = "default";

/// Table names within the bundle_info schema.
pub mod tables {
    pub static HISTORY: &str = "history";
    pub static STATUS: &str = "status";
    pub static DETAILS: &str = "details";
    pub static VIEWS: &str = "views";
    pub static INDEXES: &str = "indexes";
    pub static PACKS: &str = "packs";
    pub static BLOCKS: &str = "blocks";
    pub static CONFIG: &str = "config";
    pub static COMMANDS: &str = "commands";
    pub static CONNECTORS: &str = "connectors";
    pub static FUNCTIONS: &str = "functions";
    pub static COLUMNS: &str = "columns";
    pub static ALWAYS_DELETES: &str = "always_deletes";
    pub static ALWAYS_UPDATES: &str = "always_updates";
}

/// Type alias for the schema provider registration function.
///
/// This function is called by Bundle/BundleBuilder after creation to register
/// DataFusion schema providers with the SessionContext's catalog. The bundlebase-catalog
/// crate provides the implementation via `set_schema_provider_hook`.
pub type SchemaProviderHook = fn(&SessionContext, Weak<dyn BundleFacade>) -> Result<(), BundlebaseError>;

static SCHEMA_PROVIDER_HOOK: OnceLock<SchemaProviderHook> = OnceLock::new();

/// Set the schema provider registration hook.
///
/// Called by bundlebase-catalog (or similar) during initialization to install the
/// function that registers schema providers with DataFusion's catalog system.
/// Must be called before any Bundle or BundleBuilder is created.
pub fn set_schema_provider_hook(hook: SchemaProviderHook) {
    let _ = SCHEMA_PROVIDER_HOOK.set(hook);
}

/// Register schema providers using the installed hook, if any.
///
/// Called internally by Bundle::empty() and BundleBuilder::extend().
/// If no hook has been installed, this is a no-op.
pub(crate) fn register_schema_providers(
    ctx: &SessionContext,
    facade: Weak<dyn BundleFacade>,
) -> Result<(), BundlebaseError> {
    if let Some(hook) = SCHEMA_PROVIDER_HOOK.get() {
        hook(ctx, facade)?;
    }
    Ok(())
}
