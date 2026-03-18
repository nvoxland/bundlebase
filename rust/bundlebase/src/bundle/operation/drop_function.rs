//! DropFunction operation — removes a function definition.

use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that removes function entries by their IDs.
///
/// The entry IDs are resolved at command setup time from a function name
/// (and optional platform/signature filter). The operation itself only stores IDs,
/// making it stable across renames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropFunctionOp {
    /// IDs of the function entries to remove
    pub ids: Vec<ObjectId>,
}

impl DropFunctionOp {
    /// Resolve a function name (and optional filters) to entry IDs, creating the operation.
    pub fn setup(
        name: &str,
        platform: Option<&crate::bundle::connector_definition::Platform>,
        input_types: Option<&[arrow::datatypes::DataType]>,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let registry = builder.bundle().function_registry.read();
        let entries = registry.entries();
        let matching: Vec<_> = entries
            .iter()
            .filter(|e| {
                if e.name != name {
                    return false;
                }
                if let Some(p) = platform {
                    if &e.platform != p {
                        return false;
                    }
                }
                if let Some(types) = input_types {
                    if e.input_types != types {
                        return false;
                    }
                }
                true
            })
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "Function '{}' is not defined. Use IMPORT FUNCTION first.",
                name
            )
            .into());
        }

        let ids = matching.iter().map(|e| e.id).collect();
        Ok(Self { ids })
    }
}

#[async_trait]
impl Operation for DropFunctionOp {
    fn describe(&self) -> String {
        let id_strs: Vec<String> = self.ids.iter().map(|id| id.to_string()).collect();
        format!("DROP FUNCTION: {}", id_strs.join(", "))
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify at least one of the target IDs still exists
        let registry = bundle.function_registry.read();
        let found = self.ids.iter().any(|id| registry.entries().iter().any(|e| e.id == *id));
        if !found {
            return Err("Function entries not found. Use IMPORT FUNCTION first.".into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Look up the function name before removing entries
        let name = {
            let registry = bundle.function_registry();
            let reg = registry.read();
            reg.entries().iter()
                .find(|e| self.ids.contains(&e.id))
                .map(|e| e.name.to_string())
        };

        // Remove entries by ID
        bundle.function_registry().write().remove_by_ids(&self.ids);

        // Deregister existing UDF/UDAF and re-register remaining overloads
        if let Some(name) = name {
            let _ = bundle.ctx().deregister_udf(&name);
            let _ = bundle.ctx().deregister_udaf(&name);
            bundle.function_registry().read().register_functions_for_name(&name)
                .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::DataType;
    use crate::bundle::connector_definition::Platform;
    use crate::bundle::logic_runtime::LogicRuntime;
    use crate::bundle::function_definition::{FunctionEntry, FunctionKind};
    use crate::NamespacedName;

    #[test]
    fn test_describe() {
        let id = ObjectId::generate();
        let op = DropFunctionOp {
            ids: vec![id],
        };
        assert_eq!(op.describe(), format!("DROP FUNCTION: {}", id));
    }

    #[test]
    fn test_describe_multiple_ids() {
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        let op = DropFunctionOp {
            ids: vec![id1, id2],
        };
        assert_eq!(op.describe(), format!("DROP FUNCTION: {}, {}", id1, id2));
    }

    #[test]
    fn test_serialization() {
        let op = DropFunctionOp {
            ids: vec![ObjectId::generate()],
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropFunctionOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_not_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = DropFunctionOp {
            ids: vec![ObjectId::generate()],
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_check_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id,
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from("ipc::./test").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = DropFunctionOp {
            ids: vec![id],
        };
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_apply_removes_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id,
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from("ipc::./test").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = DropFunctionOp {
            ids: vec![id],
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.function_registry().read().has("acme.double_val"));
    }

    #[tokio::test]
    async fn test_apply_removes_by_id_preserves_others() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id: id1,
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Utf8,
            from: LogicRuntime::parse_from("ipc::./int_convert").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });
        bundle.function_registry().write().add(FunctionEntry {
            id: id2,
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Float64],
            return_type: DataType::Utf8,
            from: LogicRuntime::parse_from("ipc::./float_convert").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        // Drop only the Int64 overload by ID
        let op = DropFunctionOp {
            ids: vec![id1],
        };
        op.apply(&bundle).await.expect("apply");

        // The function should still exist (Float64 overload remains)
        assert!(bundle.function_registry().read().has("acme.convert"));
        let entries = bundle.function_registry().read().entries().to_vec();
        let convert_entries: Vec<_> = entries.iter().filter(|e| e.name.name == "convert").collect();
        assert_eq!(convert_entries.len(), 1);
        assert_eq!(convert_entries[0].input_types, vec![DataType::Float64]);
    }
}
