//! Bundle metadata UDFs for SQL queries
//!
//! This module provides DataFusion scalar UDFs that expose bundle metadata
//! within SQL queries. Currently supports:
//!
//! ```sql
//! version() -> Utf8  -- Returns the bundle's version hash
//! ```
//!
//! Example usage:
//! ```sql
//! SELECT id, name, version() AS bundle_version FROM bundle
//! ```

use arrow::array::StringArray;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, TypeSignature,
    Volatility,
};
use std::any::Any;
use std::sync::Arc;

/// The version() scalar UDF implementation.
///
/// Returns the bundle's version hash for every row in a query result.
/// This allows queries like `SELECT id, version() FROM bundle` to include
/// the bundle version alongside data.
#[derive(Debug)]
pub struct VersionUdf {
    version: Arc<String>,
    signature: Signature,
}

impl PartialEq for VersionUdf {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name() && self.version == other.version
    }
}

impl Eq for VersionUdf {}

impl std::hash::Hash for VersionUdf {
    fn hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        self.name().hash(hasher);
        self.version.hash(hasher);
    }
}

impl VersionUdf {
    /// Create a new version() UDF with the specified version string
    pub fn new(version: String) -> Self {
        Self {
            version: Arc::new(version),
            // Zero arguments, immutable (version doesn't change during query)
            signature: Signature::new(TypeSignature::Exact(vec![]), Volatility::Immutable),
        }
    }

    /// Create the UDF as a ScalarUDF
    pub fn create_udf(version: String) -> ScalarUDF {
        ScalarUDF::new_from_impl(VersionUdf::new(version))
    }
}

impl ScalarUDFImpl for VersionUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "version"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Utf8)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        // Return the version string for each row in the batch
        let num_rows = args.number_rows;
        let version_str = self.version.as_str();

        // Create an array with the version string repeated for each row
        let array = StringArray::from(vec![version_str; num_rows]);
        Ok(ColumnarValue::Array(Arc::new(array)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_udf_signature() {
        let udf = VersionUdf::new("test".to_string());

        assert_eq!(udf.name(), "version");
        assert_eq!(
            udf.return_type(&[]).expect("Should succeed"),
            DataType::Utf8
        );

        // Signature should accept zero arguments
        let sig = udf.signature();
        match &sig.type_signature {
            TypeSignature::Exact(types) => {
                assert!(types.is_empty(), "version() should take no arguments");
            }
            _ => panic!("Expected Exact signature"),
        }
    }

    #[test]
    fn test_version_udf_create() {
        let udf = VersionUdf::create_udf("v1.2.3".to_string());
        assert_eq!(udf.name(), "version");
    }
}
