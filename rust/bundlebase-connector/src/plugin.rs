//! Built-in connector implementations.

pub mod ffi;
mod http;
mod ipc;

#[cfg(feature = "connector-kaggle")]
pub mod kaggle;

#[cfg(feature = "connector-postgres")]
mod postgres;

mod remote_dir;

#[cfg(feature = "connector-web-scrape")]
mod web_scrape;

pub use ffi::FfiConnector;
pub use http::HttpConnector;
pub use ipc::IpcConnector;

#[cfg(feature = "connector-kaggle")]
pub use kaggle::KaggleConnector;

#[cfg(feature = "connector-postgres")]
pub use postgres::PostgresConnector;

pub use remote_dir::RemoteDirConnector;

#[cfg(feature = "connector-web-scrape")]
pub use web_scrape::WebScrapeConnector;
