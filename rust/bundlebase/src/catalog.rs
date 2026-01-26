mod block_schema_provider;
mod bundle_info_schema_provider;
mod default_schema_provider;
mod pack_schema_provider;
mod pack_union_table;

pub use block_schema_provider::BlockSchemaProvider;
pub use bundle_info_schema_provider::{BundleInfoSchemaProvider, BundleMetadata};
pub use default_schema_provider::DefaultSchemaProvider;
pub use pack_schema_provider::PackSchemaProvider;
pub use pack_union_table::PackUnionTable;

/// Alias dataframe is registered in the ctx under. User can select from this
pub static DATAFRAME_ALIAS: &str = "bundle";
/// Table name for bundle commit history (in bundle_info schema)
pub static BUNDLE_HISTORY_TABLE: &str = "history";
/// Table name for bundle uncommitted status (in bundle_info schema)
pub static BUNDLE_STATUS_TABLE: &str = "status";
/// Table name for bundle details (in bundle_info schema)
pub static BUNDLE_DETAILS_TABLE: &str = "details";
/// Table name for bundle views (in bundle_info schema)
pub static BUNDLE_VIEWS_TABLE: &str = "views";
/// Table name for bundle indexes (in bundle_info schema)
pub static BUNDLE_INDEXES_TABLE: &str = "indexes";
/// Table name for bundle packs (in bundle_info schema)
pub static BUNDLE_PACKS_TABLE: &str = "packs";
/// Table name for bundle blocks (in bundle_info schema)
pub static BUNDLE_BLOCKS_TABLE: &str = "blocks";
/// Datafusion catalog name used
pub static CATALOG_NAME: &str = "bundlebase";
