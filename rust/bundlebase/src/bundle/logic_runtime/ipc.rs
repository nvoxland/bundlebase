//! IPC runtime implementation.

use crate::function::ipc_bridge::SubprocessCache;
use crate::function::lib_bridge::{load_ipc_manifest, Manifest};
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};

use super::{invoke_ipc_scalar_impl, create_ipc_accumulator, LogicRuntimeImpl, RuntimeType};

/// IPC runtime: holds a path to an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcRuntime {
    pub path: String,
}

impl IpcRuntime {
    /// Parse an IPC logic string (the whole string is the path).
    pub fn parse(logic: &str) -> Result<Self, BundlebaseError> {
        if logic.is_empty() {
            return Err("IPC logic string cannot be empty".into());
        }
        Ok(Self {
            path: logic.to_string(),
        })
    }

    /// Return a new IpcRuntime with a different path.
    pub fn with_path(self, new_path: String) -> Self {
        Self { path: new_path }
    }
}

impl LogicRuntimeImpl for IpcRuntime {
    fn validate_logic(&self) -> Result<(), BundlebaseError> {
        super::validate_file_reachable(&self.path, "IPC executable")
    }

    fn can_bundle(&self) -> bool {
        true
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::Ipc
    }

    fn to_logic_string(&self) -> String {
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
