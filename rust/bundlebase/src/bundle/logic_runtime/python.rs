//! Python runtime implementation.

use crate::function::ipc_bridge::SubprocessCache;
use crate::function::python_bridge::get_python_function_bridge;
use crate::function::lib_bridge::ManifestEntry;
use crate::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};
use std::sync::Arc;

use super::{LogicRuntimeImpl, RuntimeType};

/// Python runtime: holds a module name and function/class name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonRuntime {
    pub module: String,
    pub function: String,
}

impl PythonRuntime {
    /// Parse a Python logic string like `"module:function"`.
    ///
    /// Uses the last colon as delimiter so dotted modules work
    /// (e.g., `"pkg.sub.mod:func"` → module=`"pkg.sub.mod"`, function=`"func"`).
    pub fn parse(logic: &str) -> Result<Self, BundlebaseError> {
        let colon_pos = logic.rfind(':').ok_or_else(|| {
            BundlebaseError::from(format!(
                "Invalid Python logic '{}'. Expected 'module:function' format.",
                logic
            ))
        })?;
        let module = &logic[..colon_pos];
        let function = &logic[colon_pos + 1..];

        if module.is_empty() {
            return Err(format!(
                "Invalid Python logic '{}'. Module before ':' cannot be empty.",
                logic
            ).into());
        }
        if function.is_empty() {
            return Err(format!(
                "Invalid Python logic '{}'. Function after ':' cannot be empty.",
                logic
            ).into());
        }

        Ok(Self {
            module: module.to_string(),
            function: function.to_string(),
        })
    }
}

impl LogicRuntimeImpl for PythonRuntime {
    fn can_bundle(&self) -> bool {
        false
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::Native
    }

    fn to_logic_string(&self) -> String {
        format!("{}:{}", self.module, self.function)
    }

    fn file_path(&self) -> Option<&str> {
        None
    }

    fn build_call_string(&self) -> String {
        format!("python:{}:{}", self.module, self.function)
    }

    fn lookup_function_in_manifest(
        &self,
        function_name: &str,
    ) -> Result<ManifestEntry, BundlebaseError> {
        let bridge = get_python_function_bridge()?;
        match bridge.get_function_metadata(&self.module)? {
            Some(entries) => entries
                .into_iter()
                .find(|e| e.name == function_name)
                .ok_or_else(|| {
                    format!(
                        "Function '{}' not found in manifest from '{}'. \
                         Available functions: check the manifest or provide explicit type signatures.",
                        function_name, self.to_logic_string()
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

    fn invoke_scalar(
        &self,
        name: &str,
        _function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        _subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue> {
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
                    .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
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

    fn create_accumulator(
        &self,
        name: &str,
        _function_name: &str,
        _return_type: &DataType,
        _subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>> {
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
