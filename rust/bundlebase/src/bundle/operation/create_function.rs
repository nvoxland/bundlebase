use crate::bundle::operation::Operation;
use crate::functions::FunctionSignature;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFunctionOp {
    pub signature: FunctionSignature,
}

impl PartialEq for CreateFunctionOp {
    fn eq(&self, other: &Self) -> bool {
        // Compare only the function names since FunctionSignature contains SchemaRef
        self.signature.name() == other.signature.name()
    }
}

impl CreateFunctionOp {
    pub fn setup(signature: FunctionSignature) -> Self {
        Self { signature }
    }
}

#[async_trait]
impl Operation for CreateFunctionOp {
    fn describe(&self) -> String {
        format!("CREATE FUNCTION: {}", self.signature.name())
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle
            .function_registry
            .write()
            .register(self.signature.clone())
            .map_err(|e| DataFusionError::Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow_schema::SchemaRef;

    fn create_test_schema() -> SchemaRef {
        SchemaRef::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("value", DataType::Utf8, true),
        ]))
    }

    #[test]
    fn test_describe() {
        let schema = create_test_schema();
        let op = CreateFunctionOp::setup(FunctionSignature::new("test_func", schema));
        assert_eq!(op.describe(), "CREATE FUNCTION: test_func");
    }

    #[test]
    fn test_describe_various_names() {
        let cases = vec!["my_function", "generate_data", "process_records", "f1"];

        for name in cases {
            let schema = create_test_schema();
            let op = CreateFunctionOp::setup(FunctionSignature::new(name, schema));
            assert_eq!(op.describe(), format!("CREATE FUNCTION: {}", name));
        }
    }

    #[test]
    fn test_serialization() {
        let schema = create_test_schema();
        let op = CreateFunctionOp::setup(FunctionSignature::new("test_func", schema));

        // Verify serialization is possible
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");

        // Verify we can deserialize back
        let _deserialized: CreateFunctionOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
    }

    #[test]
    fn test_config_serialization_single_field() {
        let schema = SchemaRef::new(Schema::new(vec![Field::new(
            "count",
            DataType::Int32,
            false,
        )]));
        let op = CreateFunctionOp::setup(FunctionSignature::new("count_rows", schema));

        // Verify serialization and deserialization round-trip
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: CreateFunctionOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");

        assert_eq!(deserialized.signature.name(), "count_rows");
        assert_eq!(deserialized.signature.output().fields().len(), 1);
        assert_eq!(deserialized.signature.output().field(0).name(), "count");
    }

    #[test]
    fn test_setup_name() {
        let schema = create_test_schema();
        let op = CreateFunctionOp::setup(FunctionSignature::new("my_func", schema));
        assert_eq!(op.signature.name(), "my_func");
    }

    #[test]
    fn test_setup_schema_fields() {
        let schema = SchemaRef::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("age", DataType::Int32, false),
        ]));
        let op = CreateFunctionOp::setup(FunctionSignature::new("user_data", schema.clone()));

        assert_eq!(op.signature.output().fields().len(), 3);
        assert_eq!(op.signature.output().field(0).name(), "id");
        assert_eq!(op.signature.output().field(1).name(), "name");
        assert_eq!(op.signature.output().field(2).name(), "age");
    }

    #[test]
    fn test_version() {
        let schema = create_test_schema();
        let op = CreateFunctionOp::setup(FunctionSignature::new("my_func", schema));
        let version = op.version();

        assert_eq!(version, "17d0564af14f");
    }
}
//
