//! File IO traits and helper functions.
//!
//! Trait definitions (`IOReadFile`, `IOReadWriteFile`) live in `bundlebase_common::io_file`
//! and are re-exported here for convenience.

// Re-export trait definitions from common
pub use bundlebase_common::io_file::{IOReadFile, IOReadWriteFile};

use crate::BundlebaseError;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Read file contents and deserialize from YAML.
/// Returns `None` if the file doesn't exist.
pub async fn read_yaml<T: DeserializeOwned>(
    file: &dyn IOReadFile,
) -> Result<Option<T>, BundlebaseError> {
    match file.read_str().await? {
        Some(str) => Ok(Some(serde_yaml_ng::from_str(&str)?)),
        None => Ok(None),
    }
}

/// Serialize value to YAML and write to file.
pub async fn write_yaml<T: Serialize + ?Sized>(
    file: &dyn IOReadWriteFile,
    value: &T,
) -> Result<(), BundlebaseError> {
    let yaml = serde_yaml_ng::to_string(value)?;
    file.write(Bytes::from(yaml)).await
}
