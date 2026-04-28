#![deny(clippy::unwrap_used)]

//! Indexing infrastructure for Bundlebase.
//!
//! Provides text search (via tantivy), btree indexes, row ID caching,
//! and index selection for query optimization.
//!
//! Note: `search_table_fn` (DataFusion table function integration) remains
//! in the core crate since it depends on BundleFacade.

pub mod btree_index;
mod external_sort;
mod filter_analyzer;
mod index_cache;
mod index_definition;
pub mod index_scan_exec;
mod index_selector;
mod index_trait;
mod temp_dir;
pub mod text_column_index;

#[cfg(test)]
pub(crate) mod test_utils;

pub use btree_index::{BTreeIndex, IndexedValue};
pub use external_sort::{ExternalSortConfig, ExternalSortWriter, DEFAULT_MEMORY_LIMIT_BYTES};
pub use filter_analyzer::{FilterAnalyzer, IndexPredicate, IndexableFilter};
pub use index_cache::GLOBAL_INDEX_CACHE;
pub use index_definition::{
    IndexDefinition, IndexType, IndexTypeConfigError, ParseIndexTypeError, TokenizerConfig,
};
pub use index_selector::IndexSelector;
pub use index_trait::Index;
pub use temp_dir::TempDirManager;
pub use text_column_index::{search_unified, TextIndex, TextIndexBuilder, UnifiedSearchHit};
