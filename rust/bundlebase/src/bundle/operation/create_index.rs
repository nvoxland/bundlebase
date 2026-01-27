use crate::bundle::operation::Operation;
use crate::bundle::{Bundle, BundleFacade};
use crate::index::{IndexDefinition, IndexType};
use crate::io::ObjectId;
use crate::BundlebaseError;
use arrow_schema::DataType;
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateIndexOp {
    pub column: String,
    pub id: ObjectId,
    pub index_type: IndexType,
}

impl CreateIndexOp {
    pub async fn setup(column: &str, index_type: IndexType) -> Result<Self, BundlebaseError> {
        Ok(Self {
            id: ObjectId::generate(),
            column: column.to_string(),
            index_type,
        })
    }
}

#[async_trait]
impl Operation for CreateIndexOp {
    fn describe(&self) -> String {
        match &self.index_type {
            IndexType::Column => format!("CREATE INDEX on {}", self.column),
            IndexType::Text { tokenizer } => {
                format!("CREATE TEXT INDEX on {} (tokenizer: {:?})", self.column, tokenizer)
            }
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify column exists in schema
        let schema = bundle.schema().await?;
        let field = schema
            .column_with_name(&self.column)
            .map(|(_, f)| f);

        let field = match field {
            Some(f) => f,
            None => return Err(format!("Column '{}' not found in schema", self.column).into()),
        };

        // For text indexes, verify the column is a string type
        if self.index_type.is_text() {
            match field.data_type() {
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {}
                other => {
                    return Err(format!(
                        "Text index requires a string column, but '{}' has type {:?}",
                        self.column, other
                    )
                    .into());
                }
            }
        }

        // Check if an index already exists for this column
        let indexes = bundle.indexes().read();
        if indexes.iter().any(|idx| idx.column() == &self.column) {
            return Err(format!("Index already exists for column '{}'", self.column).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle
            .indexes
            .write()
            .push(Arc::new(IndexDefinition::with_type(
                &self.id,
                &self.column,
                self.index_type.clone(),
            )));

        Ok(())
    }
}
