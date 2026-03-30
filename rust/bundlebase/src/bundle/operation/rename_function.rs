//! RenameFunction operation — renames a function definition.
//!
//! Stores the entry IDs and new name. The IDs are resolved at command setup
//! time from the old function name. Deregisters old UDFs and re-registers
//! under the new name.

use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::namespaced_name::NamespacedName;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that renames function entries and re-registers UDFs under the new name.
///
/// The entry IDs are resolved at command setup time from a function name.
/// The operation stores IDs and the new name, making it stable across
/// intermediate renames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameFunctionOp {
    /// IDs of the function entries to rename
    pub ids: Vec<ObjectId>,
    /// New function name (dotted, e.g. "acme.double_val_v2")
    pub new_name: String,
}

impl RenameFunctionOp {
    /// Resolve a function name to entry IDs and validate the rename.
    ///
    /// Checks that the old name exists, the new name is a valid namespaced name,
    /// and the new name doesn't already exist.
    pub fn setup(
        old_name: &str,
        new_name: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Validate new_name format
        let _new_namespaced = NamespacedName::parse(new_name, "Function")?;

        let registry = builder.bundle().function_registry.read();

        // Find entries matching old name
        let matching: Vec<_> = registry
            .entries()
            .iter()
            .filter(|e| e.name == old_name)
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "Function '{}' is not defined. Use IMPORT FUNCTION first.",
                old_name
            )
            .into());
        }

        // Check new name doesn't already exist
        if registry.has(new_name) {
            return Err(format!(
                "Function '{}' already exists. Drop it first or choose a different name.",
                new_name
            )
            .into());
        }

        let ids = matching.iter().map(|e| e.id).collect();
        Ok(Self {
            ids,
            new_name: new_name.to_string(),
        })
    }
}

impl Operation for RenameFunctionOp {
    fn describe(&self) -> String {
        let id_strs: Vec<String> = self.ids.iter().map(|id| id.to_string()).collect();
        format!(
            "RENAME FUNCTION: {} to '{}'",
            id_strs.join(", "),
            self.new_name
        )
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let registry = bundle.function_registry.read();

        // Verify at least one of the target IDs still exists
        let found = self
            .ids
            .iter()
            .any(|id| registry.entries().iter().any(|e| e.id == *id));
        if !found {
            return Err("Function entries not found. Use IMPORT FUNCTION first.".into());
        }

        // Check new name doesn't already exist
        if registry.has(&self.new_name) {
            return Err(format!(
                "Function '{}' already exists. Drop it first or choose a different name.",
                self.new_name
            )
            .into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let new_namespaced = NamespacedName::parse(&self.new_name, "Function")
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;

        bundle.function_registry().write().rename_by_ids(&self.ids, &new_namespaced)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::DataType;
    use crate::platform::Platform;
    use crate::bundle::function_entry::{FunctionEntry, FunctionKind};
    use crate::udf::UdfRuntime;
    use crate::NamespacedName;

    #[test]
    fn test_describe() {
        let id = ObjectId::generate();
        let op = RenameFunctionOp {
            ids: vec![id],
            new_name: "acme.double_val_v2".to_string(),
        };
        assert_eq!(
            op.describe(),
            format!("RENAME FUNCTION: {} to 'acme.double_val_v2'", id)
        );
    }

    #[test]
    fn test_describe_multiple_ids() {
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        let op = RenameFunctionOp {
            ids: vec![id1, id2],
            new_name: "acme.double_val_v2".to_string(),
        };
        assert_eq!(
            op.describe(),
            format!("RENAME FUNCTION: {}, {} to 'acme.double_val_v2'", id1, id2)
        );
    }

    #[test]
    fn test_serialization() {
        let op = RenameFunctionOp {
            ids: vec![ObjectId::generate()],
            new_name: "acme.double_val_v2".to_string(),
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: RenameFunctionOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_not_found() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = RenameFunctionOp {
            ids: vec![ObjectId::generate()],
            new_name: "acme.double_val_v2".to_string(),
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_check_found() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id,
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("ipc::./test").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = RenameFunctionOp {
            ids: vec![id],
            new_name: "acme.double_val_v2".to_string(),
        };
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_check_new_name_already_exists() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id,
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("ipc::./test").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });
        bundle.function_registry().write().add(FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "double_val_v2"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("ipc::./test2").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = RenameFunctionOp {
            ids: vec![id],
            new_name: "acme.double_val_v2".to_string(),
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_apply_renames_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id,
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("ipc::./test").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = RenameFunctionOp {
            ids: vec![id],
            new_name: "acme.double_val_v2".to_string(),
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.function_registry().read().has("acme.double_val"));
        assert!(bundle.function_registry().read().has("acme.double_val_v2"));
    }

    #[tokio::test]
    async fn test_apply_renames_all_matching_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle.function_registry().write().add(FunctionEntry {
            id: id1,
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Utf8,
            from: UdfRuntime::parse_from("ipc::./int_convert").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });
        bundle.function_registry().write().add(FunctionEntry {
            id: id2,
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Float64],
            return_type: DataType::Utf8,
            from: UdfRuntime::parse_from("ipc::./float_convert").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = RenameFunctionOp {
            ids: vec![id1, id2],
            new_name: "acme.convert_v2".to_string(),
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.function_registry().read().has("acme.convert"));
        assert!(bundle.function_registry().read().has("acme.convert_v2"));
        let entries: Vec<_> = bundle
            .function_registry()
            .read()
            .entries()
            .iter()
            .filter(|e| e.name == "acme.convert_v2")
            .cloned()
            .collect();
        assert_eq!(entries.len(), 2);
    }
}
