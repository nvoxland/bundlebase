#![deny(clippy::unwrap_used)]
extern crate core;

pub mod bundle;
mod data_reader;
pub mod data_storage;
pub mod functions;
mod index;
pub mod progress;
#[allow(clippy::unwrap_used)]
mod python;
mod schema_provider;
pub mod test_utils;
mod versioning;

pub use crate::bundle::{AnyOperation, Bundle, BundleBuilder, Operation, BundleChange, BundleStatus};
pub use crate::data_reader::DataGenerator;
pub use functions::FunctionSignature;
use std::error::Error;

/// Standard error type used throughout the Bundlebase codebase
pub type BundlebaseError = Box<dyn Error + Send + Sync>;

#[cfg(test)]
mod tests {
    // #[tokio::test]
    // fn it_works() {
    // let result = add(2, 2);
    // assert_eq!(result, 4);

    // query().await;
    // }
}
