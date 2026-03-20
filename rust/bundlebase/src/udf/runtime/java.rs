//! Java runtime implementation.

use crate::function::ipc_bridge::SubprocessCache;
use crate::function::lib_bridge::{load_java_ipc_manifest, Manifest};
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};

use super::super::entrypoint::{UdfEntrypoint, RuntimeType, validate_file_reachable};
use super::super::ipc_utils::{invoke_ipc_scalar_impl, create_ipc_accumulator};

/// Java runtime: holds a path to a JAR and an optional class name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRuntime {
    pub jar_path: String,
    pub class_name: Option<String>,
}

impl JavaRuntime {
    /// Parse a Java entrypoint string like `"./my.jar:com.example.MyClass"` or `"./my.jar"`.
    pub fn parse(entrypoint: &str) -> Result<Self, BundlebaseError> {
        if entrypoint.is_empty() {
            return Err("Java entrypoint string cannot be empty".into());
        }

        if let Some(colon_pos) = entrypoint.rfind(':') {
            let path = &entrypoint[..colon_pos];
            let class = &entrypoint[colon_pos + 1..];

            if path.is_empty() {
                return Err(format!(
                    "Invalid Java entrypoint '{}'. Path before ':' cannot be empty.",
                    entrypoint
                ).into());
            }
            if class.is_empty() {
                return Err(format!(
                    "Invalid Java entrypoint '{}'. Class after ':' cannot be empty.",
                    entrypoint
                ).into());
            }

            Ok(Self {
                jar_path: path.to_string(),
                class_name: Some(class.to_string()),
            })
        } else {
            Ok(Self {
                jar_path: entrypoint.to_string(),
                class_name: None,
            })
        }
    }

    /// Return a new JavaRuntime with a different path.
    pub fn with_path(self, new_path: String) -> Self {
        Self {
            jar_path: new_path,
            ..self
        }
    }
}

impl UdfEntrypoint for JavaRuntime {
    fn validate_entrypoint(&self) -> Result<(), BundlebaseError> {
        validate_file_reachable(&self.jar_path, "JAR file")
    }

    fn can_bundle(&self) -> bool {
        true
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::External
    }

    fn to_entrypoint_string(&self) -> String {
        match &self.class_name {
            Some(c) => format!("{}:{}", self.jar_path, c),
            None => self.jar_path.clone(),
        }
    }

    fn file_path(&self) -> Option<&str> {
        Some(&self.jar_path)
    }

    fn build_call_string(&self) -> String {
        format!("java:{}", self.to_entrypoint_string())
    }

    fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        load_java_ipc_manifest(&self.jar_path)?;
        Ok(())
    }

    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        Ok(Some(load_java_ipc_manifest(&self.jar_path)?))
    }

    fn invoke_scalar(
        &self,
        name: &str,
        _function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue> {
        invoke_ipc_scalar_impl(name, &self.to_entrypoint_string(), args, subprocess_cache)
    }

    fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &DataType,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>> {
        create_ipc_accumulator(name, &self.to_entrypoint_string(), function_name, return_type, subprocess_cache)
    }

    fn aggregate_state_type(&self, _return_type: &DataType) -> DataType {
        DataType::Utf8
    }
}
