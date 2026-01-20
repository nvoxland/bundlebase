//! Source module for source function definitions and discovery.

mod postgres;
mod remote_dir;
mod source_function;
mod source_utils;
mod web_scrape;

pub use postgres::PostgresFunction;
pub use remote_dir::RemoteDirFunction;
pub use source_function::{
    format_fetch_summary, AttachedFileInfo, FetchAction, FetchedBlock, FetchResults,
    MaterializedData, SourceFunction, SourceFunctionRegistry, SyncMode,
};
pub use web_scrape::WebScrapeFunction;
