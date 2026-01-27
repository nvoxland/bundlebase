mod blocks;
mod bundle_info;
mod default;
mod packs;

pub use blocks::BlockSchemaProvider;
pub use bundle_info::BundleInfoSchemaProvider;
pub use default::{DefaultSchemaProvider, BUNDLE_TABLE};
pub use packs::PackSchemaProvider;

/// Datafusion catalog name used
pub static CATALOG_NAME: &str = "bundlebase";
