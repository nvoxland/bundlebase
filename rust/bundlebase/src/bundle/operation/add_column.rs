use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::catalog::BundleViewTable;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use arrow::array::RecordBatch;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{SessionConfig, SessionContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AddColumnOp {

    pub id: ColumnId,
    pub name: String,
    pub expression: String,
}

impl AddColumnOp {
    pub fn setup(name: &str, expression: &str) -> Self {
        Self {
            name: name.to_string(),
            expression: expression.to_string(),
            id: ColumnId::generate(),
        }
    }
}

impl Operation for AddColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let schema = bundle.schema().await?;

        // Check column doesn't already exist
        if schema.field_with_name(&self.name).is_ok() {
            return Err(format!(
                "Column '{}' already exists in the schema",
                self.name
            )
            .into());
        }

        // Validate the expression by planning it against an empty DataFrame
        let sql = format!(
            "SELECT *, ({}) AS \"{}\" FROM bundle",
            self.expression, self.name
        );
        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let ctx = SessionContext::new_with_config(config);
        let empty_batch = RecordBatch::new_empty(schema);
        ctx.register_batch("bundle", empty_batch)
            .map_err(|e| BundlebaseError::from(format!("Failed to validate expression: {}", e)))?;
        ctx.state()
            .create_logical_plan(&sql)
            .await
            .map_err(|e| BundlebaseError::from(format!("Invalid expression '{}': {}", self.expression, e)))?;

        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
        column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        let sql = format!(
            "SELECT *, ({}) AS \"{}\" FROM bundle",
            self.expression, self.name
        );

        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let add_ctx = SessionContext::new_with_config_rt(config, ctx.runtime_env());
        add_ctx.register_table("bundle", Arc::new(BundleViewTable::new(df)))?;

        let plan = add_ctx
            .state()
            .create_logical_plan(&sql)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        let result = add_ctx
            .execute_logical_plan(plan)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        column_names.insert(self.id, self.name.clone());
        Ok(result)
    }

    fn describe(&self) -> String {
        format!("ADD COLUMN: {} AS {}", self.name, self.expression)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = AddColumnOp::setup("full_name", "first_name || ' ' || last_name");
        assert_eq!(
            op.describe(),
            "ADD COLUMN: full_name AS first_name || ' ' || last_name"
        );
    }

    #[test]
    fn test_config_serialization() {
        let op = AddColumnOp::setup("full_name", "first_name || ' ' || last_name");

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        // Serialized form includes columnId since setup() generates one
        assert!(serialized.contains("name: full_name\nexpression: first_name || ' ' || last_name"));

        let deserialized: AddColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_version_exact_value() {
        let op = AddColumnOp::setup("full_name", "first_name || ' ' || last_name");
        let version = op.version();
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);

        // Different invocations produce different versions (because of random column id)
        let op2 = AddColumnOp::setup("full_name", "first_name || ' ' || last_name");
        assert_ne!(op.version(), op2.version());
    }
}
