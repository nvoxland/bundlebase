//! Drop always-update operation — removes persistent update rules.

use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation that removes always-update rules.
///
/// If `rule_text` is `Some`, removes the specific matching rule (by "SET ... WHERE ..." text).
/// If `None`, removes all always-update rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DropAlwaysUpdateOp {
    /// The rule text to remove ("SET ... WHERE ..."), or None to remove all
    pub rule_text: Option<String>,
}

impl DropAlwaysUpdateOp {
    pub fn new(rule_text: Option<String>) -> Self {
        Self { rule_text }
    }

    pub fn drop_all() -> Self {
        Self { rule_text: None }
    }

    pub fn drop_specific(rule_text: impl Into<String>) -> Self {
        Self {
            rule_text: Some(rule_text.into()),
        }
    }
}

impl Operation for DropAlwaysUpdateOp {
    fn describe(&self) -> String {
        match &self.rule_text {
            Some(rt) => format!("DROP ALWAYS UPDATE {}", rt),
            None => "DROP ALWAYS UPDATE (all)".to_string(),
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let rules = bundle.always_update_rules();
        if rules.is_empty() {
            return Err("No always-update rules to drop".into());
        }
        if let Some(rt) = &self.rule_text {
            if !rules.iter().any(|r| r.rule_text() == *rt) {
                return Err(format!("No always-update rule matches {}", rt).into());
            }
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        match &self.rule_text {
            Some(rt) => bundle.remove_always_update_rule(rt),
            None => bundle.clear_always_update_rules(),
        }
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _column_names: &mut ColumnNames,
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
        let op = DropAlwaysUpdateOp::drop_specific("SET salary = 0 WHERE salary < 0");
        assert_eq!(op.describe(), "DROP ALWAYS UPDATE SET salary = 0 WHERE salary < 0");
    }

    #[test]
    fn test_describe_all() {
        let op = DropAlwaysUpdateOp::drop_all();
        assert_eq!(op.describe(), "DROP ALWAYS UPDATE (all)");
    }

    #[test]
    fn test_serialization() {
        let op = DropAlwaysUpdateOp::drop_specific("SET x = 1 WHERE x > 5");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: DropAlwaysUpdateOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.rule_text, Some("SET x = 1 WHERE x > 5".to_string()));
    }
}
