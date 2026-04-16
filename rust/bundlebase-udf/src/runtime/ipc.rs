//! IPC runtime implementation.

use crate::bridge::ipc_bridge::SubprocessCache;
use crate::bridge::manifest::Manifest;
use arrow::datatypes::DataType;
use bundlebase_common::BundlebaseError;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};

use super::entrypoint::{validate_file_reachable, RuntimeType, UdfEntrypoint};
use super::ipc_utils::{create_ipc_accumulator, invoke_ipc_scalar_impl};

/// IPC runtime: holds a path to an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcRuntime {
    pub path: String,
}

impl IpcRuntime {
    /// Parse an IPC entrypoint string (the whole string is the path).
    pub fn parse(entrypoint: &str) -> Result<Self, BundlebaseError> {
        if entrypoint.is_empty() {
            return Err("IPC entrypoint string cannot be empty".into());
        }
        Ok(Self {
            path: entrypoint.to_string(),
        })
    }

    /// Return a new IpcRuntime with a different path.
    pub fn with_path(self, new_path: String) -> Self {
        Self { path: new_path }
    }
}

impl UdfEntrypoint for IpcRuntime {
    fn validate_entrypoint(&self) -> Result<(), BundlebaseError> {
        // For multi-word commands (e.g., "java -cp ... ClassName"), validate only
        // the first word (the executable). Single-word paths are validated as files.
        let first_word = self.path.split_whitespace().next().unwrap_or(&self.path);
        if first_word != self.path {
            // Multi-word command: check if the executable is on PATH or is an absolute path
            if std::path::Path::new(first_word).is_absolute() {
                validate_file_reachable(first_word, "IPC executable")
            } else {
                // Assume it's on PATH (e.g., "java", "python3")
                Ok(())
            }
        } else {
            validate_file_reachable(&self.path, "IPC executable")
        }
    }

    fn can_bundle(&self) -> bool {
        true
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::External
    }

    fn to_entrypoint_string(&self) -> String {
        self.path.clone()
    }

    fn file_path(&self) -> Option<&str> {
        Some(&self.path)
    }

    fn build_call_string(&self) -> String {
        self.path.clone()
    }

    fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        load_ipc_manifest(&self.path)?;
        Ok(())
    }

    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        Ok(Some(load_ipc_manifest(&self.path)?))
    }

    fn invoke_scalar(
        &self,
        name: &str,
        _function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue> {
        invoke_ipc_scalar_impl(name, &self.path, args, subprocess_cache)
    }

    fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &DataType,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>> {
        create_ipc_accumulator(
            name,
            &self.path,
            function_name,
            return_type,
            subprocess_cache,
        )
    }

    fn aggregate_state_type(&self, _return_type: &DataType) -> DataType {
        DataType::Utf8
    }
}

/// Load a function manifest from an IPC executable.
///
/// Runs `exec_path --bundlebase-functions`, captures stdout, parses JSON.
pub(super) fn load_ipc_manifest(exec_path: &str) -> Result<Manifest, BundlebaseError> {
    let output = std::process::Command::new(exec_path)
        .arg("--bundlebase-functions")
        .output()
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to execute '{}' for manifest discovery: {}",
                exec_path, e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "'{}' --bundlebase-functions failed (exit {}): {}",
            exec_path,
            output.status,
            stderr.trim()
        )
        .into());
    }

    let json_str = String::from_utf8(output.stdout).map_err(|e| {
        BundlebaseError::from(format!(
            "Invalid UTF-8 output from '{}' --bundlebase-functions: {}",
            exec_path, e
        ))
    })?;

    let manifest: Manifest = serde_json::from_str(&json_str).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to parse manifest JSON from '{}': {}. Output: {}",
            exec_path,
            e,
            json_str.trim()
        ))
    })?;

    Ok(manifest)
}
