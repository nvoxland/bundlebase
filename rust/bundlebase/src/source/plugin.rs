//! Source function implementations (IPC, Kaggle, native, Postgres, remote_dir, web_scrape).

mod ipc;
pub(crate) mod kaggle;
pub mod native;
mod postgres;
mod remote_dir;
mod web_scrape;

pub use ipc::IpcSourceFunction;
pub use kaggle::KaggleSource;
pub use native::NativeSourceFunction;
pub use postgres::PostgresFunction;
pub use remote_dir::RemoteDirFunction;
pub use web_scrape::WebScrapeFunction;
