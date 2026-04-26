use crate::bundle::operation::Operation;
use crate::bundle::{Bundle, BundleFacade};
use crate::index::{IndexDefinition, IndexType};
use crate::io::ObjectId;
use crate::object_id::ColumnId;
use crate::BundlebaseError;
use arrow_schema::DataType;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateIndexOp {
    pub id: ObjectId,
    pub name: String,
    pub index_type: IndexType,
    pub column_ids: Vec<ColumnId>,
}

impl CreateIndexOp {
    pub async fn setup(
        column_ids: Vec<ColumnId>,
        index_type: IndexType,
        name: String,
    ) -> Result<Self, BundlebaseError> {
        // For btree indexes, validate exactly one column
        if index_type.is_btree() && column_ids.len() != 1 {
            return Err("BTree indexes must have exactly one column".into());
        }

        Ok(Self {
            id: ObjectId::generate(),
            name,
            index_type,
            column_ids,
        })
    }
}

impl Operation for CreateIndexOp {
    fn describe(&self) -> String {
        match &self.index_type {
            IndexType::BTree => format!("CREATE INDEX on column IDs: {:?}", self.column_ids),
            IndexType::Inverted { tokenizer } => {
                format!(
                    "CREATE TEXT INDEX '{}' on column IDs: {:?} (tokenizer: {:?})",
                    self.name, self.column_ids, tokenizer
                )
            }
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Use the BundleSchema (built from operations) rather than the
        // dataframe schema. This works for both populated bundles and hollow
        // bundles: the latter have no AttachBlock ops so the dataframe is
        // empty, but BundleSchema backfills column types from
        // CreateSource.expected_schema.
        let bundle_schema = bundle.bundle_schema();
        let physical = bundle_schema.physical_schema();
        let physical_ids = bundle_schema.physical_column_ids();

        // Resolve column names from IDs
        let columns: Vec<String> = self
            .column_ids
            .iter()
            .map(|id| {
                bundle.column_name(id).ok_or_else(|| {
                    BundlebaseError::from(format!("Column with ID '{}' not found", id))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let column_name_set: std::collections::HashSet<&str> =
            bundle_schema.columns().values().map(|s| s.as_str()).collect();

        if self.index_type.is_inverted() {
            // Validate all columns exist and are string types
            for (col, col_id) in columns.iter().zip(self.column_ids.iter()) {
                let field = physical_ids
                    .iter()
                    .position(|id| id == col_id)
                    .map(|i| physical.field(i));
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
                        )
                        .into());
                    }
                }
            }

            // Validate name doesn't conflict with data column names
            if column_name_set.contains(self.name.as_str()) {
                return Err(format!(
                    "Text index name '{}' conflicts with an existing data column",
                    self.name
                )
                .into());
            }

            // Check if a text index with this name already exists
            let indexes = bundle.indexes().read();
            if indexes.iter().any(|idx| idx.name() == self.name) {
                return Err(format!("Index already exists with name '{}'", self.name).into());
            }

            return Ok(());
        }

        // Single-column index validation (Column type)
        let col = &columns[0];
        let col_id = &self.column_ids[0];

        // Check if an index already exists for this column
        let indexes = bundle.indexes().read();
        if indexes.iter().any(|idx| idx.column_ids().contains(col_id)) {
            return Err(format!("Index already exists for column '{}'", col).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.indexes.write().push(Arc::new(IndexDefinition::new(
            &self.id,
            self.name.clone(),
            self.index_type.clone(),
            self.column_ids.clone(),
        )));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_btree_index_rejects_multiple_columns() {
        let result = CreateIndexOp::setup(
            vec![ColumnId::generate(), ColumnId::generate()],
            IndexType::BTree,
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
