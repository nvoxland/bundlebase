mod block_schema_provider;
mod bundle_schema_provider;
mod pack_schema_provider;
mod pack_union_table;

pub use block_schema_provider::BlockSchemaProvider;
pub use bundle_schema_provider::BundleSchemaProvider;
pub use pack_schema_provider::PackSchemaProvider;
pub use pack_union_table::PackUnionTable;

/// Alias dataframe is registered in the ctx under. User can select from this
pub static DATAFRAME_ALIAS: &str = "bundle";
/// Datafusion catalog name used
pub static CATALOG_NAME: &str = "bundlebase";
