// Re-export everything from the bundlebase-index crate
pub use bundlebase_index::*;

// search_table_fn stays in core since it depends on BundleFacade
mod search_table_fn;
pub use search_table_fn::SearchTableFunction;
