use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::catalog::BundleViewTable;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{SessionConfig, SessionContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CastColumnOp {
    pub id: ColumnId,
    pub new_type: String,
    pub clean: Option<String>,
}

impl CastColumnOp {
    pub fn setup(id: ColumnId, new_type: &str, clean: Option<String>) -> Self {
        Self {
            id,
            new_type: new_type.to_string(),
            clean,
        }
    }
}

/// Parse a user-facing type string into (Arrow DataType, SQL type string).
fn parse_target_type(type_str: &str) -> Result<&'static str, BundlebaseError> {
    match type_str.to_lowercase().as_str() {
        "integer" | "int" => Ok("BIGINT"),
        "float" | "double" => Ok("DOUBLE"),
        "string" | "text" => Ok("VARCHAR"),
        "boolean" | "bool" => Ok("BOOLEAN"),
        "date" => Ok("DATE"),
        "timestamp" => Ok("TIMESTAMP"),
        _ => Err(format!(
            "Unsupported target type '{}'. Valid types: integer, int, float, double, string, text, boolean, bool, date, timestamp",
            type_str
        ).into()),
    }
}

#[async_trait]
impl Operation for CastColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?;

        // Validate target type
        parse_target_type(&self.new_type)?;

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
        ctx: Arc<SessionContext>,
        column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        let sql_type = parse_target_type(&self.new_type)?;
        let schema = df.schema().clone();

        // Resolve the column name from the column names map
        let name = column_names.get(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?
            .clone();

        // Build SELECT expression list
        let mut select_exprs: Vec<String> = Vec::new();
        for field in schema.fields() {
            let field_name = field.name();
            if field_name == &name {
                let quoted = format!("\"{}\"", field_name);
                let expr = if let Some(ref pattern) = self.clean {
                    // Escape single quotes in the pattern for SQL
                    let escaped_pattern = pattern.replace('\'', "''");
                    format!(
                        "CAST(regexp_replace({}, '{}', '', 'g') AS {}) AS \"{}\"",
                        quoted, escaped_pattern, sql_type, field_name
                    )
                } else {
                    format!("CAST({} AS {}) AS \"{}\"", quoted, sql_type, field_name)
                };
                select_exprs.push(expr);
            } else {
                select_exprs.push(format!("\"{}\"", field_name));
            }
        }

        let sql = format!("SELECT {} FROM bundle", select_exprs.join(", "));

        // Use isolated SessionContext pattern from FilterOp
        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let cast_ctx = SessionContext::new_with_config_rt(config, ctx.runtime_env());
        cast_ctx.register_table("bundle", Arc::new(BundleViewTable::new(df)))?;

        let plan = cast_ctx
            .state()
            .create_logical_plan(&sql)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        cast_ctx
            .execute_logical_plan(plan)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)
    }

    fn describe(&self) -> String {
        match &self.clean {
            Some(pattern) => format!(
                "CAST COLUMN: {} to {} (clean: {})",
                self.id, self.new_type, pattern
            ),
            None => format!(
                "CAST COLUMN: {} to {}",
                self.id, self.new_type
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CastColumnOp::setup(ColumnId::generate(), "integer", None);
        assert!(op.describe().contains("to integer"));
    }

    #[test]
    fn test_describe_with_clean() {
        let op = CastColumnOp::setup(ColumnId::generate(), "integer", Some("[^0-9]".to_string()));
        assert!(op.describe().contains("to integer"));
        assert!(op.describe().contains("clean: [^0-9]"));
    }

    #[test]
    fn test_config_serialization() {
        let id = ColumnId::generate();
        let op = CastColumnOp::setup(id, "integer", Some("[^0-9]".to_string()));

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("newType: integer\nclean: '[^0-9]'"));

        let deserialized: CastColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_config_serialization_no_clean() {
        let op = CastColumnOp::setup(ColumnId::generate(), "integer", None);

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: CastColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_parse_target_type_valid() {
        assert_eq!(parse_target_type("integer").unwrap(), "BIGINT");
        assert_eq!(parse_target_type("int").unwrap(), "BIGINT");
        assert_eq!(parse_target_type("float").unwrap(), "DOUBLE");
        assert_eq!(parse_target_type("double").unwrap(), "DOUBLE");
        assert_eq!(parse_target_type("string").unwrap(), "VARCHAR");
        assert_eq!(parse_target_type("text").unwrap(), "VARCHAR");
        assert_eq!(parse_target_type("boolean").unwrap(), "BOOLEAN");
        assert_eq!(parse_target_type("bool").unwrap(), "BOOLEAN");
        assert_eq!(parse_target_type("date").unwrap(), "DATE");
        assert_eq!(parse_target_type("timestamp").unwrap(), "TIMESTAMP");
    }

    #[test]
    fn test_parse_target_type_case_insensitive() {
        assert_eq!(parse_target_type("INTEGER").unwrap(), "BIGINT");
        assert_eq!(parse_target_type("Float").unwrap(), "DOUBLE");
    }

    #[test]
    fn test_parse_target_type_invalid() {
        let result = parse_target_type("invalid_type");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported target type"));
        assert!(err.contains("invalid_type"));
    }

    #[test]
    fn test_version_exact_value() {
        let op = CastColumnOp::setup(ColumnId::generate(), "integer", None);
        let version = op.version();
        // Version now includes column id so it varies per invocation
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);
    }
}
