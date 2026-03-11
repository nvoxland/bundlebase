//! ImportFunction operation — registers a named function definition.

use crate::bundle::connector_definition::{Platform, Runner};
use crate::bundle::function_definition::{arrow_type_serde, parse_function_name, FunctionEntry, FunctionKind};
use crate::data::ObjectId;
use crate::NamespacedName;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use arrow::datatypes::DataType;
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that defines a named function and registers it with DataFusion.
///
/// Always persisted — for runtime-only functions, use `import_temp_function` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportFunctionOp {
    /// Unique identifier for this function entry
    pub id: ObjectId,
    /// Full dotted function name (e.g., "acme.double_val")
    pub name: String,
    /// Arrow types for input parameters
    #[serde(with = "arrow_type_serde::vec")]
    pub input_types: Vec<DataType>,
    /// Arrow type for the return value
    #[serde(with = "arrow_type_serde::single")]
    pub return_type: DataType,
    /// Runner type
    pub runner: Runner,
    /// Logic string (e.g., path to binary or module:function)
    pub logic: String,
    /// Platform pattern in Docker-style os/arch
    pub platform: Platform,
    /// Scalar or aggregate
    pub kind: FunctionKind,
}

impl ImportFunctionOp {
    pub fn new(
        name: String,
        input_types: Vec<DataType>,
        return_type: DataType,
        runner: Runner,
        logic: String,
        platform: Platform,
        kind: FunctionKind,
    ) -> Self {
        Self { id: ObjectId::generate(), name, input_types, return_type, runner, logic, platform, kind }
    }
}

#[async_trait]
impl Operation for ImportFunctionOp {
    fn describe(&self) -> String {
        let input_strs: Vec<String> = self.input_types.iter().map(|dt| dt.to_string()).collect();
        format!(
            "IMPORT FUNCTION {}({}) RETURNS {} (runner={}, platform={})",
            self.name,
            input_strs.join(", "),
            self.return_type,
            self.runner,
            self.platform
        )
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Validate name has exactly one dot
        parse_function_name(&self.name)?;

        // Reject python runner (cannot be bundled)
        if self.runner == Runner::Python {
            return Err(
                "python runner cannot be bundled. Use IMPORT TEMP FUNCTION instead.".into(),
            );
        }

        // Types are already validated DataType values — no parsing needed

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let namespaced = self.name.parse::<NamespacedName>()
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        let entry = FunctionEntry {
            id: self.id,
            name: namespaced,
            input_types: self.input_types.clone(),
            return_type: self.return_type.clone(),
            runner: self.runner,
            logic: self.logic.clone(),
            platform: self.platform.clone(),
            temporary: false,
            kind: self.kind,
        };
        // Warn if overwriting an existing definition
        if bundle.has_function_entry(&self.name) {
            tracing::warn!("Overwriting existing function definition for '{}'", self.name);
        }

        // Add to registry first so resolve_all can find all overloads
        bundle.add_function_entry(entry);
        bundle.register_functions_for_name(&self.name)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = ImportFunctionOp::new(
            "acme.double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            Runner::Ipc,
            "./my_func".to_string(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        assert_eq!(
            op.describe(),
            "IMPORT FUNCTION acme.double_val(Int64) RETURNS Int64 (runner=ipc, platform=*/*)"
        );
    }

    #[test]
    fn test_serialization() {
        let op = ImportFunctionOp::new(
            "acme.double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            Runner::Ipc,
            "./my_func".to_string(),
            "linux/amd64".parse().unwrap(),
            FunctionKind::Scalar,
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: ImportFunctionOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[test]
    fn test_serialization_aggregate() {
        let op = ImportFunctionOp::new(
            "acme.my_sum".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            Runner::Ipc,
            "./my_sum".to_string(),
            Platform::any(),
            FunctionKind::Aggregate,
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: ImportFunctionOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
        assert_eq!(deser.kind, FunctionKind::Aggregate);
    }

    #[tokio::test]
    async fn test_check_no_dot() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = ImportFunctionOp::new(
            "double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            Runner::Ipc,
            "./test".to_string(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be in format 'namespace.name'"));
    }

    #[tokio::test]
    async fn test_check_python_rejected() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = ImportFunctionOp::new(
            "acme.double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            Runner::Python,
            "mod:func".to_string(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("python runner cannot be bundled"));
    }

    #[tokio::test]
    async fn test_apply_registers_entry() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = ImportFunctionOp::new(
            "acme.double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            Runner::Ipc,
            "./my_func".to_string(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        op.apply(&bundle).await.expect("apply");
        assert!(bundle.has_function_entry("acme.double_val"));
    }
}
