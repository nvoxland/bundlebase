//! Built-in connector implementations (IPC, Kaggle, FFI, Postgres, remote_dir, web_scrape).

pub mod ffi;
mod ipc;
pub(crate) mod kaggle;
mod postgres;
mod remote_dir;
mod web_scrape;

pub use ffi::FfiConnector;
pub use ipc::IpcConnector;
pub use kaggle::KaggleConnector;
pub use postgres::PostgresConnector;
pub use remote_dir::RemoteDirConnector;
pub use web_scrape::WebScrapeConnector;
