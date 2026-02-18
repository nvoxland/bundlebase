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
    pub columns: Vec<String>,
    pub id: ObjectId,
    pub name: String,
    pub index_type: IndexType,
}

impl CreateIndexOp {
    pub async fn setup(columns: Vec<String>, index_type: IndexType, name: String) -> Result<Self, BundlebaseError> {
        // For column indexes, validate exactly one column
        if index_type.is_column() && columns.len() != 1 {
            return Err("Column indexes must have exactly one column".into());
        }

        Ok(Self {
            id: ObjectId::generate(),
            columns,
            name,
            index_type,
        })
    }
}

#[async_trait]
impl Operation for CreateIndexOp {
    fn describe(&self) -> String {
        match &self.index_type {
            IndexType::Column => format!("CREATE INDEX on {}", self.columns.join(", ")),
            IndexType::Text { tokenizer } => {
                let cols = self.columns.join(", ");
                format!("CREATE TEXT INDEX '{}' on [{}] (tokenizer: {:?})", self.name, cols, tokenizer)
            }
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let schema = bundle.schema().await?;

        if self.index_type.is_text() {
            // Validate all columns exist and are string types
            for col in &self.columns {
                let field = schema.column_with_name(col).map(|(_, f)| f);
                let field = match field {
                    Some(f) => f,
                    None => return Err(format!("Column '{}' not found in schema", col).into()),
                };
                match field.data_type() {
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {}
                    other => {
                        return Err(format!(
                            "Text index requires a string column, but '{}' has type {:?}",
                            col, other
                        ).into());
                    }
                }
            }

            // Validate name doesn't conflict with data column names
            if schema.column_with_name(&self.name).is_some() {
                return Err(format!(
                    "Text index name '{}' conflicts with an existing data column",
                    self.name
                ).into());
            }

            // Check if a text index with this name already exists
            let indexes = bundle.indexes().read();
            if indexes.iter().any(|idx| idx.name() == self.name) {
                return Err(format!("Index already exists with name '{}'", self.name).into());
            }

            return Ok(());
        }

        // Single-column index validation (Column type)
        let col = &self.columns[0];
        let field = schema
            .column_with_name(col)
            .map(|(_, f)| f);

        match field {
            Some(_) => {}
            None => return Err(format!("Column '{}' not found in schema", col).into()),
        };

        // Check if an index already exists for this column
        let indexes = bundle.indexes().read();
        if indexes.iter().any(|idx| idx.columns().contains(col)) {
            return Err(format!("Index already exists for column '{}'", col).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle
            .indexes
            .write()
            .push(Arc::new(IndexDefinition::new(
                &self.id,
                self.name.clone(),
                self.columns.clone(),
                self.index_type.clone(),
            )));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_column_index_rejects_multiple_columns() {
        let result = CreateIndexOp::setup(
            vec!["col1".to_string(), "col2".to_string()],
            IndexType::Column,
            "test_idx".to_string(),
        )
        .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exactly one column"));
    }
}
