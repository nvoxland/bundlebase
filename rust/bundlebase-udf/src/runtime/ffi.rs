//! FFI (shared library) runtime implementation.

use crate::bridge::ipc_bridge::SubprocessCache;
use crate::bridge::ffi_bridge::{
    invoke_lib_scalar, load_lib_manifest, LibAccumulator,
};
use crate::bridge::manifest::Manifest;
use bundlebase_common::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};
use std::sync::Arc;

use super::entrypoint::{UdfEntrypoint, RuntimeType, validate_file_reachable};

/// FFI runtime: holds a path to a shared library and an optional symbol name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiRuntime {
    pub path: String,
    pub symbol: Option<String>,
}

impl FfiRuntime {
    /// Parse an FFI entrypoint string like `"./mylib.so:double_val"` or `"./mylib.so"`.
    pub fn parse(entrypoint: &str) -> Result<Self, BundlebaseError> {
        if entrypoint.is_empty() {
            return Err("FFI entrypoint string cannot be empty".into());
        }

        if let Some(colon_pos) = entrypoint.rfind(':') {
            let path = &entrypoint[..colon_pos];
            let symbol = &entrypoint[colon_pos + 1..];

            if path.is_empty() {
                return Err(format!(
                    "Invalid FFI entrypoint '{}'. Path before ':' cannot be empty.",
                    entrypoint
                ).into());
            }
            if symbol.is_empty() {
                return Err(format!(
                    "Invalid FFI entrypoint '{}'. Symbol after ':' cannot be empty.",
                    entrypoint
                ).into());
            }

            Ok(Self {
                path: path.to_string(),
                symbol: Some(symbol.to_string()),
            })
        } else {
            Ok(Self {
                path: entrypoint.to_string(),
                symbol: None,
            })
        }
    }

    /// Return a new FfiRuntime with a different path.
    pub fn with_path(self, new_path: String) -> Self {
        Self {
            path: new_path,
            ..self
        }
    }
}

impl UdfEntrypoint for FfiRuntime {
    fn validate_entrypoint(&self) -> Result<(), BundlebaseError> {
        validate_file_reachable(&self.path, "Shared library")
    }

    fn can_bundle(&self) -> bool {
        true
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::Internal
    }

    fn to_entrypoint_string(&self) -> String {
        match &self.symbol {
            Some(s) => format!("{}:{}", self.path, s),
            None => self.path.clone(),
        }
    }

    fn file_path(&self) -> Option<&str> {
        Some(&self.path)
    }

    fn build_call_string(&self) -> String {
        format!("ffi:{}", self.to_entrypoint_string())
    }

    fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        load_lib_manifest(&self.path)?;
        Ok(())
    }

    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        Ok(Some(load_lib_manifest(&self.path)?))
    }

    fn invoke_scalar(
        &self,
        name: &str,
        function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        _subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue> {
        let symbol = self.symbol.as_deref().unwrap_or(function_name);

        let arrays: Vec<ArrayRef> = args
            .args
            .iter()
            .map(|cv| match cv {
                ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
                ColumnarValue::Scalar(scalar) => scalar
                    .to_array_of_size(args.number_rows)
                    .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
            })
            .collect::<DFResult<Vec<_>>>()?;

        let result = invoke_lib_scalar(&self.path, symbol, &arrays).map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "FFI function '{}' ({}:{}) failed: {}",
                name, self.path, symbol, e
            ))
        })?;

        Ok(ColumnarValue::Array(result))
    }

    fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &DataType,
        _subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>> {
        let symbol = self.symbol.as_deref().unwrap_or(function_name);

        let acc = LibAccumulator::new(&self.path, symbol, return_type.clone()).map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "Failed to create FFI accumulator for '{}': {}",
                name, e
            ))
        })?;
        Ok(Box::new(acc))
    }
}
