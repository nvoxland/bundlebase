//! Always-delete operation — registers a persistent delete rule.
//!
//! `AlwaysDeleteOp` stores a WHERE clause that is automatically applied
//! to newly attached data. The rule persists across commits and reopens.

use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation that registers a persistent always-delete rule.
///
/// The WHERE clause is stored and automatically applied to data
/// attached after this operation. On bundle open, the rule is
/// added to `Bundle.always_delete_rules`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlwaysDeleteOp {
    /// The WHERE clause condition (without the "WHERE" keyword)
    #[serde(rename = "where")]
    pub where_clause: String,
}

impl AlwaysDeleteOp {
    pub fn new(where_clause: impl Into<String>) -> Self {
        Self {
            where_clause: where_clause.into(),
        }
    }
}

impl Operation for AlwaysDeleteOp {
    fn describe(&self) -> String {
        format!("ALWAYS DELETE WHERE {}", self.where_clause)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.where_clause.trim().is_empty() {
            return Err("ALWAYS DELETE WHERE clause cannot be empty".into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.add_always_delete_rule(&self.where_clause);
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        // No-op: always-delete rules apply at attach time, not query time
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
    fn test_describe() {
        let op = AlwaysDeleteOp::new("salary < 0");
        assert_eq!(op.describe(), "ALWAYS DELETE WHERE salary < 0");
    }

    #[test]
    fn test_serialization() {
        let op = AlwaysDeleteOp::new("status = 'inactive'");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(yaml.contains("where:"));
        assert!(yaml.contains("status = 'inactive'"));

        let deserialized: AlwaysDeleteOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.where_clause, "status = 'inactive'");
    }
}
