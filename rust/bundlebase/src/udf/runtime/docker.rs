//! Docker runtime implementation.

use crate::function::ipc_bridge::SubprocessCache;
use crate::function::manifest::Manifest;
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};

use super::super::entrypoint::{UdfEntrypoint, RuntimeType};
use super::super::ipc_utils::{invoke_ipc_scalar_impl, create_ipc_accumulator};
use super::ipc::load_ipc_manifest;

/// Docker runtime: holds an image name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerRuntime {
    pub image: String,
}

impl DockerRuntime {
    /// Parse a Docker entrypoint string (the whole string is the image name).
    pub fn parse(entrypoint: &str) -> Result<Self, BundlebaseError> {
        if entrypoint.is_empty() {
            return Err("Docker entrypoint string cannot be empty".into());
        }
        Ok(Self {
            image: entrypoint.to_string(),
        })
    }
}

impl UdfEntrypoint for DockerRuntime {
    fn can_bundle(&self) -> bool {
        true
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::External
    }

    fn to_entrypoint_string(&self) -> String {
        self.image.clone()
    }

    fn file_path(&self) -> Option<&str> {
        None
    }

    fn build_call_string(&self) -> String {
        format!("docker:{}", self.image)
    }

    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        Ok(Some(load_ipc_manifest(&self.image)?))
    }

    fn invoke_scalar(
        &self,
        name: &str,
        _function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue> {
        invoke_ipc_scalar_impl(name, &self.image, args, subprocess_cache)
    }

    fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &DataType,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>> {
        create_ipc_accumulator(name, &self.image, function_name, return_type, subprocess_cache)
    }

    fn aggregate_state_type(&self, _return_type: &DataType) -> DataType {
        DataType::Utf8
    }
}
