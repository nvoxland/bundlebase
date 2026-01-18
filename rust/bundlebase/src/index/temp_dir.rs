//! Temporary directory management for index building.
//!
//! Provides a unified interface for creating temporary directories that works
//! with both local and remote storage backends.

use crate::io::IOReadWriteDir;
use crate::BundlebaseError;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

/// Manages temporary directory for index building operations.
///
/// This struct handles temporary directory creation and cleanup for operations
/// like external sorting during index building. The strategy depends on the
/// storage backend:
///
/// - **Local storage** (file://, memory://, tar://): Creates temp directory at
///   `data_dir/temp/<prefix>_<random>`. This keeps temp files on the same
///   filesystem as the data for better performance.
///
/// - **Remote storage** (s3://, gs://, azure://): Uses system temp directory
///   at `$TEMP/bundlebase_<prefix>_<random>`. Remote storage doesn't benefit
///   from co-location and system temp is typically faster.
///
/// The directory is automatically cleaned up when `TempDirManager` is dropped.
pub struct TempDirManager {
    /// System temp dir handle (when using remote storage) - cleaned up on drop
    _system_temp: Option<TempDir>,
    /// Path to the temp directory
    path: PathBuf,
    /// Whether this is a local data_dir temp (needs manual cleanup)
    is_local_data_dir: bool,
}

impl TempDirManager {
    /// Create a new temporary directory manager.
    ///
    /// # Arguments
    /// * `data_dir` - The data directory for the bundle
    /// * `prefix` - Prefix for the temp directory name (e.g., "column_index")
    ///
    /// # Returns
    /// A `TempDirManager` that will clean up the directory on drop.
    ///
    /// # Errors
    /// Returns an error if the temp directory cannot be created.
    pub fn new(data_dir: &Arc<dyn IOReadWriteDir>, prefix: &str) -> Result<Self, BundlebaseError> {
        if data_dir.is_local_storage() {
            Self::create_local_temp(data_dir, prefix)
        } else {
            Self::create_system_temp(prefix)
        }
    }

    /// Create temp directory within the local data_dir.
    fn create_local_temp(
        data_dir: &Arc<dyn IOReadWriteDir>,
        prefix: &str,
    ) -> Result<Self, BundlebaseError> {
        // Create temp subdirectory within data_dir
        let temp_subdir = data_dir.writable_subdir("temp").map_err(|e| {
            BundlebaseError::from(format!("Failed to create temp subdir: {}", e))
        })?;

        // Generate unique name with prefix and random suffix
        let unique_name = format!("{}_{:016x}", prefix, rand::random::<u64>());
        let build_dir = temp_subdir.writable_subdir(&unique_name).map_err(|e| {
            BundlebaseError::from(format!("Failed to create build dir: {}", e))
        })?;

        // Extract local path from file:// URL
        let url = build_dir.url();
        let path = if url.scheme() == "file" {
            PathBuf::from(url.path())
        } else {
            // For memory:// or tar://, use system temp as fallback
            return Self::create_system_temp(prefix);
        };

        // Create the directory on the filesystem
        std::fs::create_dir_all(&path).map_err(|e| {
            BundlebaseError::from(format!("Failed to create temp dir at {:?}: {}", path, e))
        })?;

        Ok(Self {
            _system_temp: None,
            path,
            is_local_data_dir: true,
        })
    }

    /// Create temp directory using system temp.
    fn create_system_temp(prefix: &str) -> Result<Self, BundlebaseError> {
        let temp_dir = TempDir::with_prefix(&format!("bundlebase_{}_", prefix)).map_err(|e| {
            BundlebaseError::from(format!("Failed to create system temp dir: {}", e))
        })?;
        let path = temp_dir.path().to_path_buf();

        Ok(Self {
            _system_temp: Some(temp_dir),
            path,
            is_local_data_dir: false,
        })
    }

    /// Get the path to the temporary directory.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempDirManager {
    fn drop(&mut self) {
        // System temp: TempDir handles cleanup automatically via _system_temp
        // Local data_dir temp: clean up manually
        if self.is_local_data_dir {
            if let Err(e) = std::fs::remove_dir_all(&self.path) {
                // Best effort cleanup - log but don't panic
                log::warn!("Failed to clean up temp directory {:?}: {}", self.path, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::random_memory_dir;

    #[test]
    fn test_memory_dir_uses_system_temp() {
        // Memory URLs are considered "local" but don't have a real filesystem path,
        // so they fall back to system temp
        let data_dir = random_memory_dir();
        let temp_manager = TempDirManager::new(&data_dir, "test").unwrap();

        // Verify path exists and is in system temp
        assert!(temp_manager.path().exists());
        assert!(temp_manager.path().is_dir());

        let path_str = temp_manager.path().to_string_lossy();
        assert!(path_str.contains("bundlebase_test_"));
    }

    #[test]
    fn test_system_temp_cleanup_on_drop() {
        let path: PathBuf;

        {
            let temp_manager = TempDirManager::create_system_temp("cleanup_test").unwrap();
            path = temp_manager.path().clone();
            assert!(path.exists());
        }

        // After drop, directory should be cleaned up by TempDir
        assert!(!path.exists());
    }

    #[test]
    fn test_system_temp_creation() {
        let temp_manager = TempDirManager::create_system_temp("system_test").unwrap();

        assert!(temp_manager.path().exists());
        assert!(temp_manager.path().is_dir());

        let path_str = temp_manager.path().to_string_lossy();
        assert!(path_str.contains("bundlebase_system_test_"));
    }
}
