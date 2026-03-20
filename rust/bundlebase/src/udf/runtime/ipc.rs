//! IPC runtime implementation.

use crate::function::ipc_bridge::SubprocessCache;
use crate::function::lib_bridge::{load_ipc_manifest, Manifest};
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};

use super::super::entrypoint::{UdfEntrypoint, RuntimeType, validate_file_reachable};
use super::super::ipc_utils::{invoke_ipc_scalar_impl, create_ipc_accumulator};

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
        validate_file_reachable(&self.path, "IPC executable")
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
        create_ipc_accumulator(name, &self.path, function_name, return_type, subprocess_cache)
    }

    fn aggregate_state_type(&self, _return_type: &DataType) -> DataType {
        DataType::Utf8
    }
}
