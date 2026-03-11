//! DropFunction operation — removes a function definition.

use crate::bundle::connector_definition::Platform;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use arrow::datatypes::DataType;
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that removes a function definition and all associated entries,
/// or removes only entries for a specific platform or input type signature.
///
/// If both `platform` and `input_types` are None, the entire function is removed.
/// If `platform` is Some, only entries for that platform are removed.
/// If `input_types` is Some, only entries matching that signature are removed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropFunctionOp {
    /// Full dotted function name (e.g., "acme.double_val")
    pub name: String,
    /// Optional platform filter. None means all platforms.
    pub platform: Option<Platform>,
    /// Optional input type signature filter. None means all signatures.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "option_arrow_types")]
    pub input_types: Option<Vec<DataType>>,
}

/// Serde helpers for Optional Vec<DataType>.
mod option_arrow_types {
    use crate::bundle::function_definition::arrow_type_serde;
    use arrow::datatypes::DataType;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<Vec<DataType>>, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer {
        match value {
            Some(types) => arrow_type_serde::vec::serialize(types, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<DataType>>, D::Error>
    where D: Deserializer<'de> {
        use serde::Deserialize;
        let opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(None),
            Some(strings) => {
                let types = strings.iter()
                    .map(|s| crate::bundle::function_definition::parse_arrow_type_name(s).map_err(serde::de::Error::custom))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(types))
            }
        }
    }
}

impl DropFunctionOp {
    pub fn new(name: String, platform: Option<Platform>) -> Self {
        Self { name, platform, input_types: None }
    }

    pub fn new_with_signature(name: String, platform: Option<Platform>, input_types: Option<Vec<DataType>>) -> Self {
        Self { name, platform, input_types }
    }
}

#[async_trait]
impl Operation for DropFunctionOp {
    fn describe(&self) -> String {
        let sig = match &self.input_types {
            Some(types) => {
                let type_strs: Vec<String> = types.iter().map(|dt| dt.to_string()).collect();
                format!("({})", type_strs.join(", "))
            }
            None => String::new(),
        };
        match &self.platform {
            Some(p) => format!("DROP FUNCTION {}{} FOR PLATFORM '{}'", self.name, sig, p),
            None => format!("DROP FUNCTION {}{}", self.name, sig),
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        if !bundle.has_function_entry(&self.name) {
            return Err(format!(
                "Function '{}' is not defined. Use IMPORT FUNCTION first.",
                self.name
            )
            .into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Remove entries from registry
        match (&self.platform, &self.input_types) {
            (None, None) => {
                // Drop all entries for this name
                bundle
                    .remove_function_entries(&self.name)
                    .map_err(|e| DataFusionError::Execution(e.to_string()))?;
            }
            (Some(ref p), _) => {
                // Drop by platform (existing behavior)
                bundle
                    .remove_function_entry(&self.name, Some(p), false)
                    .map_err(|e| DataFusionError::Execution(e.to_string()))?;
            }
            (None, Some(ref types)) => {
                // Drop by input type signature
                bundle.remove_function_entries_by_signature(&self.name, types);
            }
        }

        // Deregister existing UDF/UDAF and re-register remaining overloads
        let _ = bundle.ctx().deregister_udf(&self.name);
        let _ = bundle.ctx().deregister_udaf(&self.name);
        bundle
            .register_functions_for_name(&self.name)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::DataType;
    use crate::bundle::connector_definition::{Platform, Runner};
    use crate::bundle::function_definition::{FunctionEntry, FunctionKind};
    use crate::NamespacedName;

    #[test]
    fn test_describe_without_platform() {
        let op = DropFunctionOp::new("acme.double_val".to_string(), None);
        assert_eq!(op.describe(), "DROP FUNCTION acme.double_val");
    }

    #[test]
    fn test_describe_with_platform() {
        let op = DropFunctionOp::new(
            "acme.double_val".to_string(),
            Some("linux/amd64".parse().unwrap()),
        );
        assert_eq!(
            op.describe(),
            "DROP FUNCTION acme.double_val FOR PLATFORM 'linux/amd64'"
        );
    }

    #[test]
    fn test_describe_with_input_types() {
        let op = DropFunctionOp::new_with_signature(
            "acme.double_val".to_string(),
            None,
            Some(vec![DataType::Int64]),
        );
        assert_eq!(op.describe(), "DROP FUNCTION acme.double_val(Int64)");
    }

    #[test]
    fn test_serialization() {
        let op = DropFunctionOp::new("acme.double_val".to_string(), None);
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropFunctionOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[test]
    fn test_serialization_with_input_types() {
        let op = DropFunctionOp::new_with_signature(
            "acme.add".to_string(),
            None,
            Some(vec![DataType::Int64, DataType::Int64]),
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropFunctionOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_not_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = DropFunctionOp::new("acme.double_val".to_string(), None);
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not defined"));
    }

    #[tokio::test]
    async fn test_check_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_function_entry(FunctionEntry {
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Ipc,
            logic: "./test".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = DropFunctionOp::new("acme.double_val".to_string(), None);
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_apply_removes_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_function_entry(FunctionEntry {
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Ipc,
            logic: "./test".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        let op = DropFunctionOp::new("acme.double_val".to_string(), None);
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.has_function_entry("acme.double_val"));
    }

    #[tokio::test]
    async fn test_apply_removes_by_signature() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_function_entry(FunctionEntry {
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Utf8,
            runner: Runner::Ipc,
            logic: "./int_convert".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });
        bundle.add_function_entry(FunctionEntry {
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Float64],
            return_type: DataType::Utf8,
            runner: Runner::Ipc,
            logic: "./float_convert".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        });

        // Drop only the Int64 overload
        let op = DropFunctionOp::new_with_signature(
            "acme.convert".to_string(),
            None,
            Some(vec![DataType::Int64]),
        );
        op.apply(&bundle).await.expect("apply");

        // The function should still exist (Float64 overload remains)
        assert!(bundle.has_function_entry("acme.convert"));
        let entries = bundle.function_entries();
        let convert_entries: Vec<_> = entries.iter().filter(|e| e.name.name == "convert").collect();
        assert_eq!(convert_entries.len(), 1);
        assert_eq!(convert_entries[0].input_types, vec![DataType::Float64]);
    }
}
