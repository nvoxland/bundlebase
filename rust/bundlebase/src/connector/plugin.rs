//! Built-in connector implementations (IPC, Kaggle, native, Postgres, remote_dir, web_scrape).

mod ipc;
pub(crate) mod kaggle;
pub mod native;
mod postgres;
mod remote_dir;
mod web_scrape;

pub use ipc::IpcConnector;
pub use kaggle::KaggleConnector;
pub use native::NativeConnector;
pub use postgres::PostgresConnector;
pub use remote_dir::RemoteDirConnector;
pub use web_scrape::WebScrapeConnector;
