use crate::bundle::operation::Operation;
use crate::bundle::{Bundle, BundleFacade};
use crate::index::IndexDefinition;
use crate::io::ObjectId;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateIndexOp {
    pub column: String,
    pub id: ObjectId,
}

impl CreateIndexOp {
    pub async fn setup(column: &str) -> Result<Self, BundlebaseError> {
        Ok(Self {
            id: ObjectId::generate(),
            column: column.to_string(),
        })
    }
}

#[async_trait]
impl Operation for CreateIndexOp {
    fn describe(&self) -> String {
        format!("CREATE INDEX on {}", self.column)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify column exists in schema
        if !bundle
            .schema()
            .await?
            .column_with_name(&self.column)
            .is_some()
        {
            return Err(format!("Column '{}' not found in schema", self.column).into());
        }

        // Check if an index already exists for this column
        let indexes = bundle.indexes().read();
        if indexes.iter().any(|idx| idx.column() == &self.column) {
            return Err(format!("Index already exists for column '{}'", self.column).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        bundle
            .indexes
            .write()
            .push(Arc::new(IndexDefinition::new(&self.id, &self.column)));

        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_define_index_config_serialization() {
//         let config = DefineIndexOpConfig {
//             column: "salary".to_string(),
//             index_file: Some("01-abc123.salary.colidx".to_string()),
//             cardinality: Some(1000),
//         };
//
//         let json = serde_json::to_string(&config).unwrap();
//         let deserialized: DefineIndexOpConfig = serde_json::from_str(&json).unwrap();
//
//         assert_eq!(config, deserialized);
//     }
//
//     #[test]
//     fn test_define_index_describe() {
//         let config = DefineIndexOpConfig {
//             column: "email".to_string(),
//             index_file: None,
//             cardinality: None,
//         };
//
//         assert_eq!(config.describe(), "CREATE INDEX on column 'email'");
//     }
// }
