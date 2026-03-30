use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use arrow_schema::DataType;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::logical_expr::{Expr, expr::Cast};
use datafusion::common::Column;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CastColumnOp {
    pub id: ColumnId,
    pub new_type: DataType,
}

impl CastColumnOp {
    pub fn setup(id: ColumnId, new_type: DataType) -> Self {
        Self {
            id,
            new_type,
        }
    }
}

impl Operation for CastColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?;

        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        let schema = df.schema().clone();
        let internal_name = bundle_schema.internal_name(&self.id)?;

        let mut select_exprs: Vec<Expr> = Vec::new();
        for field in schema.fields() {
            let field_name = field.name();
            if field_name == &internal_name {
                let cast_expr = Expr::Cast(Cast {
                    expr: Box::new(Expr::Column(Column::new_unqualified(field_name))),
                    data_type: self.new_type.clone(),
                });
                select_exprs.push(cast_expr.alias(field_name.as_str()));
            } else {
                select_exprs.push(Expr::Column(Column::new_unqualified(field_name)));
            }
        }

        df.select(select_exprs)
            .map_err(|e| Box::new(e) as BundlebaseError)
    }

    fn describe(&self) -> String {
        format!(
            "CAST COLUMN: {} to {}",
            self.id, self.new_type
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CastColumnOp::setup(ColumnId::generate(), DataType::Int64);
        assert!(op.describe().contains("to Int64"));
    }

    #[test]
    fn test_config_serialization() {
        let id = ColumnId::generate();
        let op = CastColumnOp::setup(id, DataType::Int64);

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: CastColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_version_exact_value() {
        let op = CastColumnOp::setup(ColumnId::generate(), DataType::Int64);
        let version = op.version();
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);
    }
}
