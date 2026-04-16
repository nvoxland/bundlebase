#![deny(clippy::unwrap_used)]

mod blocks;
mod bundle_info;
mod default;
mod packs;

pub use blocks::BlockSchemaProvider;
pub use bundle_info::BundleInfoSchemaProvider;
pub use default::DefaultSchemaProvider;
pub use packs::PackSchemaProvider;

// Re-export constants from core for convenience
pub use bundlebase::catalog::tables;
pub use bundlebase::catalog::{BUNDLE_INFO_SCHEMA, BUNDLE_TABLE, CATALOG_NAME, DEFAULT_SCHEMA};

use bundlebase::bundle::BundleFacade;
use bundlebase_common::BundlebaseError;
use datafusion::catalog::MemorySchemaProvider;
use datafusion::prelude::SessionContext;
use std::sync::{Arc, Weak};

/// Install the catalog schema provider hook into bundlebase core.
///
/// This must be called before creating any Bundle or BundleBuilder to ensure
/// schema providers are registered. Typically called once at application startup.
///
/// After calling this, all Bundle/BundleBuilder creation will automatically
/// register the catalog schema providers (blocks, packs, default, bundle_info).
pub fn init() {
    bundlebase::catalog::set_schema_provider_hook(register_schema_providers);
}

/// Register schema providers with the SessionContext's catalog.
///
/// Creates all schema providers with the facade reference and registers them
/// with the catalog. This must be called after Bundle/BundleBuilder is wrapped
/// in Arc.
pub fn register_schema_providers(
    ctx: &SessionContext,
    facade: Weak<dyn BundleFacade>,
) -> Result<(), BundlebaseError> {
    let catalog = ctx
        .catalog(CATALOG_NAME)
        .expect("Default catalog not found");

    // Register temp schema (doesn't need facade)
    catalog.register_schema("temp", Arc::new(MemorySchemaProvider::new()))?;

    catalog.register_schema("blocks", Arc::new(BlockSchemaProvider::new(facade.clone())))?;
    catalog.register_schema("packs", Arc::new(PackSchemaProvider::new(facade.clone())))?;
    catalog.register_schema(
        DEFAULT_SCHEMA,
        Arc::new(DefaultSchemaProvider::new(facade.clone())),
    )?;
    catalog.register_schema(
        BUNDLE_INFO_SCHEMA,
        Arc::new(BundleInfoSchemaProvider::new(facade)),
    )?;

    Ok(())
}

/// Register schema providers for a BundleFacade implementor wrapped in Arc.
///
/// This is a convenience wrapper around `register_schema_providers` that
/// extracts the SessionContext and creates the Weak reference automatically.
pub fn register_catalog<T: BundleFacade + 'static>(facade: &Arc<T>) -> Result<(), BundlebaseError> {
    let ctx = facade.ctx();
    let weak = Arc::downgrade(facade) as Weak<dyn BundleFacade>;
    register_schema_providers(&ctx, weak)
}

/// Register schema providers for a type-erased `Arc<dyn BundleFacade>`.
///
/// This is a convenience wrapper for when you have a `dyn BundleFacade`.
pub fn register_catalog_dyn(facade: &Arc<dyn BundleFacade>) -> Result<(), BundlebaseError> {
    let ctx = facade.ctx();
    let weak = Arc::downgrade(facade);
    register_schema_providers(&ctx, weak)
}
