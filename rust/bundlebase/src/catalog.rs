mod block_schema_provider;
mod bundle_info_schema_provider;
mod default_schema_provider;
mod pack_schema_provider;
mod pack_union_table;

pub use block_schema_provider::BlockSchemaProvider;
pub use bundle_info_schema_provider::BundleInfoSchemaProvider;
pub use default_schema_provider::DefaultSchemaProvider;
pub use pack_schema_provider::PackSchemaProvider;
pub use pack_union_table::PackUnionTable;

/// Alias dataframe is registered in the ctx under. User can select from this
pub static DATAFRAME_ALIAS: &str = "bundle";
/// Table name for bundle commit history (in bundle_info schema)
pub static BUNDLE_HISTORY_TABLE: &str = "history";
/// Datafusion catalog name used
pub static CATALOG_NAME: &str = "bundlebase";
