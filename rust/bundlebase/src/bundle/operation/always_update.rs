//! Always-update operation — registers a persistent update rule.
//!
//! `AlwaysUpdateOp` stores SET assignments and a WHERE clause that are
//! automatically applied to newly attached data. The rule persists
//! across commits and reopens.

use crate::bundle::AlwaysUpdateRule;
use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation that registers a persistent always-update rule.
///
/// The SET clause and WHERE clause are stored and automatically applied
/// to data attached after this operation. On bundle open, the rule is
/// added to `Bundle.always_update_rules`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlwaysUpdateOp {
    /// The SET clause (without the "SET" keyword), e.g. "salary = 0, status = 'inactive'"
    #[serde(rename = "set")]
    pub set_clause: String,
    /// The WHERE clause condition (without the "WHERE" keyword)
    #[serde(rename = "where")]
    pub where_clause: String,
}

impl AlwaysUpdateOp {
    pub fn new(set_clause: impl Into<String>, where_clause: impl Into<String>) -> Self {
        Self {
            set_clause: set_clause.into(),
            where_clause: where_clause.into(),
        }
    }
}

impl Operation for AlwaysUpdateOp {
    fn describe(&self) -> String {
        format!("ALWAYS UPDATE SET {} WHERE {}", self.set_clause, self.where_clause)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.set_clause.trim().is_empty() {
            return Err("ALWAYS UPDATE SET clause cannot be empty".into());
        }
        if self.where_clause.trim().is_empty() {
            return Err("ALWAYS UPDATE WHERE clause cannot be empty".into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let rule = AlwaysUpdateRule::new(&self.set_clause, &self.where_clause);
        bundle.add_always_update_rule(&rule);
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        // No-op: always-update rules apply at attach time, not query time
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
        let op = AlwaysUpdateOp::new("salary = 0", "salary < 0");
        assert_eq!(op.describe(), "ALWAYS UPDATE SET salary = 0 WHERE salary < 0");
    }

    #[test]
    fn test_serialization() {
        let op = AlwaysUpdateOp::new("status = 'inactive'", "last_login < '2020-01-01'");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(yaml.contains("set:"));
        assert!(yaml.contains("where:"));
        assert!(yaml.contains("status = 'inactive'"));

        let deserialized: AlwaysUpdateOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.set_clause, "status = 'inactive'");
        assert_eq!(deserialized.where_clause, "last_login < '2020-01-01'");
    }
}
