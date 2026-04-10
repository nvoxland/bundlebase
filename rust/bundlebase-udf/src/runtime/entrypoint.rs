//! UDF entrypoint trait and shared helpers.

use async_trait::async_trait;
use crate::bridge::ipc_bridge::SubprocessCache;
pub use crate::bridge::manifest::{Manifest, ManifestEntry};
use bundlebase_io::IOReadWriteDir;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};
use std::sync::Arc;

/// The type of connector registry used by a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeType {
    /// Internal (in-process) execution via FFI shared libraries or Python bridge.
    Internal,
    /// External (out-of-process) execution via subprocess JSON-RPC protocol.
    External,
}

/// Trait for runtime-specific behavior.
///
/// Each runtime struct holds parsed fields and implements this trait,
/// so methods can use own fields directly instead of re-parsing entrypoint strings.
#[async_trait]
pub trait UdfEntrypoint: Send + Sync + std::fmt::Debug {
    /// Whether this runtime's entrypoint can be persisted in a bundle.
    fn can_bundle(&self) -> bool;

    /// The type of connector registry used by this runtime.
    fn runtime_type(&self) -> RuntimeType;

    /// Reconstruct the entrypoint portion of the FROM string.
    fn to_entrypoint_string(&self) -> String;

    /// Return the file path if this runtime references a local file.
    fn file_path(&self) -> Option<&str>;

    /// Build the prefixed call string for IPC/native dispatch.
    fn build_call_string(&self) -> String;

    /// Validate that the referenced entrypoint (file, module, etc.) is reachable.
    ///
    /// Called at import time to fail fast if the entrypoint doesn't exist.
    /// Default implementation is a no-op (e.g., Docker images are validated at run time).
    fn validate_entrypoint(&self) -> Result<(), BundlebaseError> {
        Ok(())
    }

    /// Verify this runtime's bundled artifact is loadable (e.g., load manifest from shared lib).
    fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        Ok(())
    }

    /// Load the function manifest for wildcard discovery.
    /// Returns None if this runtime doesn't support wildcard discovery.
    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        Ok(None)
    }

    /// Look up a single function's metadata from manifest.
    /// Default implementation loads the manifest and searches it.
    /// Runtimes that don't use manifests (e.g., Python) should override.
    fn lookup_function_in_manifest(
        &self,
        function_name: &str,
    ) -> Result<ManifestEntry, BundlebaseError> {
        let manifest = self.load_manifest()?.ok_or_else(|| -> BundlebaseError {
            format!(
                "Function discovery not supported for this runtime (entrypoint: '{}')",
                self.to_entrypoint_string()
            )
            .into()
        })?;
        find_in_manifest(manifest, function_name, &self.to_entrypoint_string())
    }

    /// Invoke a scalar function.
    fn invoke_scalar(
        &self,
        name: &str,
        function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue>;

    /// Create an accumulator for an aggregate function.
    fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &DataType,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>>;

    /// DataType for aggregate state serialization.
    /// IPC runtimes use Utf8 (opaque state ID), others use return type.
    fn aggregate_state_type(&self, return_type: &DataType) -> DataType {
        return_type.clone()
    }

    /// Copy the file referenced by this runtime into the bundle's data directory.
    ///
    /// Returns the new bundle-relative path, or `None` if no copy is needed
    /// (i.e., the runtime doesn't reference a local file).
    async fn copy_into_bundle(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<Option<String>, BundlebaseError> {
        let file_path = match self.file_path() {
            Some(p) => p.to_string(),
            None => return Ok(None),
        };

        let abs_path = if file_path.starts_with('/') {
            std::path::PathBuf::from(&file_path)
        } else {
            std::env::current_dir()
                .map_err(|e| {
                    BundlebaseError::from(format!("Failed to get current directory: {}", e))
                })?
                .join(&file_path)
        };

        let file_bytes = tokio::fs::read(&abs_path).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read file '{}': {}",
                abs_path.display(),
                e
            ))
        })?;

        let ext_str = abs_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let format = bundlebase_common::ContentFormat::from_extension(ext_str)
            .unwrap_or(bundlebase_common::ContentFormat::Bin);
        let address = bundlebase_common::ContentAddress::new(
            bundlebase_common::ContentCategory::Udf,
            format,
        );

        let stream = futures::stream::once(async move {
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(file_bytes))
        });
        let write_result = data_dir.write_stream(Box::pin(stream), &address).await?;

        // Preserve execute permissions for binary files (connectors, scripts)
        let url = write_result.file.url();
        if url.scheme() == "file" {
            if let Ok(path) = url.to_file_path() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = std::fs::metadata(&path) {
                        let mut perms = metadata.permissions();
                        perms.set_mode(perms.mode() | 0o111); // add execute
                        let _ = std::fs::set_permissions(&path, perms);
                    }
                }
            }
        }

        let hash = &write_result.hash;
        let bundle_path = format!("{}/{}.{}", &hash[..2], &hash[2..16], address.extension());

        Ok(Some(bundle_path))
    }
}

// ==================== Shared helpers ====================

/// Check that a file-based entrypoint path exists on disk.
///
/// Resolves relative paths against the current working directory.
/// Returns a descriptive error if the file is not found.
pub(crate) fn validate_file_reachable(path: &str, label: &str) -> Result<(), BundlebaseError> {
    let abs = if path.starts_with('/') {
        std::path::PathBuf::from(path)
    } else {
        std::env::current_dir()
            .map_err(|e| BundlebaseError::from(format!("Failed to get current directory: {}", e)))?
            .join(path)
    };
    if !abs.exists() {
        return Err(format!(
            "{} not found: '{}' (resolved to '{}')",
            label,
            path,
            abs.display()
        )
        .into());
    }
    Ok(())
}

/// Look up a function by name in a manifest, or return a descriptive error.
pub(crate) fn find_in_manifest(
    manifest: Manifest,
    function_name: &str,
    entrypoint: &str,
) -> Result<ManifestEntry, BundlebaseError> {
    let available_names: Vec<String> = manifest.functions.iter().map(|e| e.name.clone()).collect();

    manifest
        .functions
        .into_iter()
        .find(|e| e.name == function_name)
        .ok_or_else(|| {
            if available_names.is_empty() {
                format!(
                    "Function '{}' not found in manifest from '{}'. \
                     The manifest contains no functions.",
                    function_name, entrypoint
                )
            } else {
                format!(
                    "Function '{}' not found in manifest from '{}'. \
                     Available functions: {}",
                    function_name, entrypoint, available_names.join(", ")
                )
            }
            .into()
        })
}
