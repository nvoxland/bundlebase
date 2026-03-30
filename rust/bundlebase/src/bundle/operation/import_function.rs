//! ImportFunction operation — registers a named function definition.

use crate::platform::Platform;
use crate::udf::UdfRuntime;
use crate::bundle::function_entry::{parse_function_name, FunctionEntry, FunctionKind};
use crate::data::ObjectId;
use crate::NamespacedName;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use arrow::datatypes::DataType;
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
    pub input_types: Vec<DataType>,
    /// Arrow type for the return value
    pub return_type: DataType,
    /// Runtime with parsed entrypoint (e.g., `ipc::./my_func`)
    pub from: UdfRuntime,
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
        from: UdfRuntime,
        platform: Platform,
        kind: FunctionKind,
    ) -> Self {
        Self { id: ObjectId::generate(), name, input_types, return_type, from, platform, kind }
    }
}

impl Operation for ImportFunctionOp {
    fn describe(&self) -> String {
        let input_strs: Vec<String> = self.input_types.iter().map(|dt| dt.to_string()).collect();
        format!(
            "IMPORT FUNCTION {}({}) RETURNS {} (runtime={}, platform={})",
            self.name,
            input_strs.join(", "),
            self.return_type,
            self.from.runtime_name(),
            self.platform
        )
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        parse_function_name(&self.name)?;
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
            from: self.from.clone(),
            platform: self.platform.clone(),
            temporary: false,
            kind: self.kind,
        };
        if bundle.function_registry().read().has(&self.name) {
            tracing::warn!("Overwriting existing function definition for '{}'", self.name);
        }

        bundle.function_registry().write().add_and_register(entry)
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
            UdfRuntime::parse_from("ipc::./my_func").unwrap(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        assert_eq!(
            op.describe(),
            "IMPORT FUNCTION acme.double_val(Int64) RETURNS Int64 (runtime=ipc, platform=*/*)"
        );
    }

    #[test]
    fn test_serialization() {
        let op = ImportFunctionOp::new(
            "acme.double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            UdfRuntime::parse_from("ipc::./my_func").unwrap(),
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
            UdfRuntime::parse_from("ipc::./my_sum").unwrap(),
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
            UdfRuntime::parse_from("ipc::./test").unwrap(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be in format 'namespace.name'"));
    }

    #[tokio::test]
    async fn test_apply_registers_entry() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = ImportFunctionOp::new(
            "acme.double_val".to_string(),
            vec![DataType::Int64],
            DataType::Int64,
            UdfRuntime::parse_from("ipc::./my_func").unwrap(),
            Platform::any(),
            FunctionKind::Scalar,
        );
        op.apply(&bundle).await.expect("apply");
        assert!(bundle.function_registry().read().has("acme.double_val"));
    }
}
