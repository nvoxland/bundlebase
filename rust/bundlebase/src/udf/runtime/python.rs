//! Python runtime implementation.
//!
//! Supports two entrypoint styles:
//! - **Module-backed** (`python::mymodule:func`): in-process via PyO3 bridge, NOT bundleable
//! - **File-backed** (`python::./script.py:func`): `.py` file on disk, bundleable, executed via IPC subprocess

use crate::function::ipc_bridge::SubprocessCache;
use crate::function::python_bridge::get_python_function_bridge;
use crate::function::manifest::{ManifestEntry, Manifest};
use crate::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};
use std::sync::Arc;

use super::super::entrypoint::{validate_file_reachable, UdfEntrypoint, RuntimeType};
use super::super::ipc_utils::create_ipc_accumulator;

/// Python runtime: holds a module name and function/class name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonRuntime {
    pub module: String,
    pub function: String,
}

impl PythonRuntime {
    /// Parse a Python entrypoint string like `"module:function"`.
    ///
    /// Uses the last colon as delimiter so dotted modules work
    /// (e.g., `"pkg.sub.mod:func"` → module=`"pkg.sub.mod"`, function=`"func"`).
    pub fn parse(entrypoint: &str) -> Result<Self, BundlebaseError> {
        let colon_pos = entrypoint.rfind(':').ok_or_else(|| {
            BundlebaseError::from(format!(
                "Invalid Python entrypoint '{}'. Expected 'module:function' format.",
                entrypoint
            ))
        })?;
        let module = &entrypoint[..colon_pos];
        let function = &entrypoint[colon_pos + 1..];

        if module.is_empty() {
            return Err(format!(
                "Invalid Python entrypoint '{}'. Module before ':' cannot be empty.",
                entrypoint
            ).into());
        }
        if function.is_empty() {
            return Err(format!(
                "Invalid Python entrypoint '{}'. Function after ':' cannot be empty.",
                entrypoint
            ).into());
        }

        Ok(Self {
            module: module.to_string(),
            function: function.to_string(),
        })
    }

    /// Whether this entrypoint references a `.py` file on disk (file-backed)
    /// rather than an importable Python module (module-backed).
    ///
    /// Detection: if the module portion contains `/` or ends with `.py`.
    pub fn is_file_backed(&self) -> bool {
        self.module.contains('/') || self.module.ends_with(".py")
    }

    /// Return a new PythonRuntime with a different module path.
    ///
    /// Only meaningful for file-backed entrypoints (used by `copy_into_bundle`).
    pub fn with_path(self, new_path: String) -> Self {
        Self {
            module: new_path,
            ..self
        }
    }
}

impl UdfEntrypoint for PythonRuntime {
    fn validate_entrypoint(&self) -> Result<(), BundlebaseError> {
        if self.is_file_backed() {
            validate_file_reachable(&self.module, "Python script")
        } else {
            let bridge = get_python_function_bridge()?;
            // If get_function_metadata succeeds (even returning None for no metadata),
            // the module is importable. An import error will propagate as Err.
            let _ = bridge.get_function_metadata(&self.module)?;
            Ok(())
        }
    }

    fn can_bundle(&self) -> bool {
        self.is_file_backed()
    }

    fn runtime_type(&self) -> RuntimeType {
        if self.is_file_backed() {
            RuntimeType::External
        } else {
            RuntimeType::Internal
        }
    }

    fn to_entrypoint_string(&self) -> String {
        format!("{}:{}", self.module, self.function)
    }

    fn file_path(&self) -> Option<&str> {
        if self.is_file_backed() {
            Some(&self.module)
        } else {
            None
        }
    }

    fn build_call_string(&self) -> String {
        if self.is_file_backed() {
            // IPC harness: `python -m bundlebase_sdk._ipc_harness <script.py>`
            // parse_call splits on whitespace, so space-delimited is correct.
            format!(
                "python -m bundlebase_sdk._ipc_harness {}",
                self.module
            )
        } else {
            format!("python:{}:{}", self.module, self.function)
        }
    }

    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        if self.is_file_backed() {
            Ok(Some(load_python_ipc_manifest(&self.module)?))
        } else {
            Ok(None)
        }
    }

    fn lookup_function_in_manifest(
        &self,
        function_name: &str,
    ) -> Result<ManifestEntry, BundlebaseError> {
        if self.is_file_backed() {
            // Use default trait impl (spawns subprocess with --bundlebase-functions)
            let manifest = self.load_manifest()?.ok_or_else(|| -> BundlebaseError {
                format!(
                    "Function discovery not supported for this runtime (entrypoint: '{}')",
                    self.to_entrypoint_string()
                )
                .into()
            })?;
            super::super::entrypoint::find_in_manifest(
                manifest,
                function_name,
                &self.to_entrypoint_string(),
            )
        } else {
            let bridge = get_python_function_bridge()?;
            match bridge.get_function_metadata(&self.module)? {
                Some(entries) => entries
                    .into_iter()
                    .find(|e| e.name == function_name)
                    .ok_or_else(|| {
                        format!(
                            "Function '{}' not found in manifest from '{}'. \
                             Available functions: check the manifest or provide explicit type signatures.",
                            function_name, self.to_entrypoint_string()
                        )
                        .into()
                    }),
                None => Err(format!(
                    "Python module '{}' does not define bundlebase_metadata(). \
                     Provide explicit type signatures or add a bundlebase_metadata() function.",
                    self.module
                )
                .into()),
            }
        }
    }

    fn invoke_scalar(
        &self,
        name: &str,
        _function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue> {
        if self.is_file_backed() {
            // Use self.function (the Python symbol name from the entrypoint) as
            // the IPC function name, not the user-facing display name.
            let call_string = self.build_call_string();
            let arrays: Vec<ArrayRef> = args
                .args
                .iter()
                .map(|cv| match cv {
                    ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
                    ColumnarValue::Scalar(scalar) => scalar
                        .to_array_of_size(args.number_rows)
                        .map_err(|e| {
                            datafusion::common::DataFusionError::Execution(e.to_string())
                        }),
                })
                .collect::<DFResult<Vec<_>>>()?;

            let result = crate::function::ipc_bridge::invoke_ipc_scalar(
                subprocess_cache,
                &call_string,
                &self.function,
                &arrays,
            )
            .map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "IPC function '{}' ({}) failed: {}",
                    name, call_string, e
                ))
            })?;

            Ok(ColumnarValue::Array(result))
        } else {
            let bridge = get_python_function_bridge().map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Cannot invoke Python function '{}': {}",
                    name, e
                ))
            })?;

            let arrays: Vec<ArrayRef> = args
                .args
                .iter()
                .map(|cv| match cv {
                    ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
                    ColumnarValue::Scalar(scalar) => scalar
                        .to_array_of_size(args.number_rows)
                        .map_err(|e| {
                            datafusion::common::DataFusionError::Execution(e.to_string())
                        }),
                })
                .collect::<DFResult<Vec<_>>>()?;

            let result = bridge
                .invoke(&self.module, &self.function, &arrays, args.number_rows)
                .map_err(|e| {
                    datafusion::common::DataFusionError::Execution(format!(
                        "Python function '{}' ({}:{}) failed: {}",
                        name, self.module, self.function, e
                    ))
                })?;

            Ok(ColumnarValue::Array(result))
        }
    }

    fn create_accumulator(
        &self,
        name: &str,
        _function_name: &str,
        return_type: &DataType,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>> {
        if self.is_file_backed() {
            // Use self.function (the Python class name from the entrypoint) as
            // the IPC function name, not the user-facing display name.
            let call_string = self.build_call_string();
            create_ipc_accumulator(
                name,
                &call_string,
                &self.function,
                return_type,
                subprocess_cache,
            )
        } else {
            let bridge = get_python_function_bridge().map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Cannot create accumulator for '{}': {}",
                    name, e
                ))
            })?;

            let initial_state = bridge
                .aggregate_create_state(&self.module, &self.function)
                .map_err(|e| {
                    datafusion::common::DataFusionError::Execution(format!(
                        "Failed to create initial state for '{}': {}",
                        name, e
                    ))
                })?;

            Ok(Box::new(crate::function::aggregate::PythonAccumulator {
                module: self.module.clone(),
                class_name: self.function.clone(),
                state: initial_state,
                function_name: name.to_string(),
            }))
        }
    }

    fn aggregate_state_type(&self, return_type: &DataType) -> DataType {
        if self.is_file_backed() {
            DataType::Utf8
        } else {
            return_type.clone()
        }
    }
}

/// Load a function manifest from a Python script via IPC harness.
///
/// Runs `python -m bundlebase_sdk._ipc_harness <script.py> --bundlebase-functions`,
/// captures stdout, parses JSON.
fn load_python_ipc_manifest(script_path: &str) -> Result<Manifest, BundlebaseError> {
    let output = std::process::Command::new("python")
        .args([
            "-m",
            "bundlebase_sdk._ipc_harness",
            script_path,
            "--bundlebase-functions",
        ])
        .output()
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to execute 'python -m bundlebase_sdk._ipc_harness {}' for manifest discovery: {}",
                script_path, e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "'python -m bundlebase_sdk._ipc_harness {}' --bundlebase-functions failed (exit {}): {}",
            script_path,
            output.status,
            stderr.trim()
        )
        .into());
    }

    let json_str = String::from_utf8(output.stdout).map_err(|e| {
        BundlebaseError::from(format!(
            "Invalid UTF-8 output from Python IPC harness for '{}': {}",
            script_path, e
        ))
    })?;

    let manifest: Manifest = serde_json::from_str(&json_str).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to parse manifest JSON from Python script '{}': {}. Output: {}",
            script_path, e, json_str.trim()
        ))
    })?;

    Ok(manifest)
}
