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
}
