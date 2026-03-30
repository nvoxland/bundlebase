//! Drop always-delete operation — removes persistent delete rules.

use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation that removes always-delete rules.
///
/// If `where_clause` is `Some`, removes the specific matching rule.
/// If `None`, removes all always-delete rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DropAlwaysDeleteOp {
    /// The WHERE clause to remove, or None to remove all rules
    pub where_clause: Option<String>,
}

impl DropAlwaysDeleteOp {
    pub fn new(where_clause: Option<String>) -> Self {
        Self { where_clause }
    }

    pub fn drop_all() -> Self {
        Self { where_clause: None }
    }

    pub fn drop_specific(where_clause: impl Into<String>) -> Self {
        Self {
            where_clause: Some(where_clause.into()),
        }
    }
}

impl Operation for DropAlwaysDeleteOp {
    fn describe(&self) -> String {
        match &self.where_clause {
            Some(wc) => format!("DROP ALWAYS DELETE WHERE {}", wc),
            None => "DROP ALWAYS DELETE (all)".to_string(),
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let rules = bundle.always_delete_rules();
        if rules.is_empty() {
            return Err("No always-delete rules to drop".into());
        }
        if let Some(wc) = &self.where_clause {
            if !rules.contains(wc) {
                return Err(format!("No always-delete rule matches WHERE {}", wc).into());
            }
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        match &self.where_clause {
            Some(wc) => bundle.remove_always_delete_rule(wc),
            None => bundle.clear_always_delete_rules(),
        }
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        Ok(df)
    }

    fn allowed_on_view(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe_specific() {
        let op = DropAlwaysDeleteOp::drop_specific("salary < 0");
        assert_eq!(op.describe(), "DROP ALWAYS DELETE WHERE salary < 0");
    }

    #[test]
    fn test_describe_all() {
        let op = DropAlwaysDeleteOp::drop_all();
        assert_eq!(op.describe(), "DROP ALWAYS DELETE (all)");
    }

    #[test]
    fn test_serialization() {
        let op = DropAlwaysDeleteOp::drop_specific("x > 5");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: DropAlwaysDeleteOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.where_clause, Some("x > 5".to_string()));
    }
}
