use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::function_definition::{arrow_type_serde, arrow_type_to_name};
use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use arrow_schema::DataType;
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::logical_expr::{Expr, expr::Cast};
use datafusion::common::Column;
use datafusion::prelude::{lit, SessionContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CastColumnOp {
    pub id: ColumnId,
    #[serde(with = "arrow_type_serde::single")]
    pub new_type: DataType,
    pub clean: Option<String>,
}

impl CastColumnOp {
    pub fn setup(id: ColumnId, new_type: DataType, clean: Option<String>) -> Self {
        Self {
            id,
            new_type,
            clean,
        }
    }
}

#[async_trait]
impl Operation for CastColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?;

        // Validate clean regex if provided
        if let Some(ref pattern) = self.clean {
            regex::Regex::new(pattern).map_err(|e| -> BundlebaseError {
                format!("Invalid clean regex '{}': {}", pattern, e).into()
            })?;
        }

        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        let schema = df.schema().clone();

        // Resolve the column name from the column names map
        let name = column_names.get(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?
            .clone();

        // Build SELECT expression list
        let mut select_exprs: Vec<Expr> = Vec::new();
        for field in schema.fields() {
            let field_name = field.name();
            if field_name == &name {
                let base_expr = if let Some(ref pattern) = self.clean {
                    let func = datafusion::functions::regex::regexp_replace();
                    Expr::ScalarFunction(datafusion::logical_expr::expr::ScalarFunction {
                        func,
                        args: vec![
                            Expr::Column(Column::new_unqualified(field_name)),
                            lit(pattern.as_str()),
                            lit(""),
                            lit("g"),
                        ],
                    })
                } else {
                    Expr::Column(Column::new_unqualified(field_name))
                };

                let cast_expr = Expr::Cast(Cast {
                    expr: Box::new(base_expr),
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
        let type_name = arrow_type_to_name(&self.new_type);
        match &self.clean {
            Some(pattern) => format!(
                "CAST COLUMN: {} to {} (clean: {})",
                self.id, type_name, pattern
            ),
            None => format!(
                "CAST COLUMN: {} to {}",
                self.id, type_name
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CastColumnOp::setup(ColumnId::generate(), DataType::Int64, None);
        assert!(op.describe().contains("to Int64"));
    }

    #[test]
    fn test_describe_with_clean() {
        let op = CastColumnOp::setup(ColumnId::generate(), DataType::Int64, Some("[^0-9]".to_string()));
        assert!(op.describe().contains("to Int64"));
        assert!(op.describe().contains("clean: [^0-9]"));
    }

    #[test]
    fn test_config_serialization() {
        let id = ColumnId::generate();
        let op = CastColumnOp::setup(id, DataType::Int64, Some("[^0-9]".to_string()));

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("newType: Int64\nclean: '[^0-9]'"));

        let deserialized: CastColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_config_serialization_no_clean() {
        let op = CastColumnOp::setup(ColumnId::generate(), DataType::Int64, None);

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: CastColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_version_exact_value() {
        let op = CastColumnOp::setup(ColumnId::generate(), DataType::Int64, None);
        let version = op.version();
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);
    }
}
