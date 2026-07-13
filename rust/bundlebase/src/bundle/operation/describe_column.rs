use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Attach a human/agent-readable description to a column, keyed by `ColumnId`
/// so it survives renames. Metadata-only: it records semantic context that
/// travels in the manifest and is surfaced to agents (via the MCP `describe`
/// tool) and to callers — it does not transform the data. The description is
/// resolved by replaying operations (last write wins per column), so no
/// separate durable storage is needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DescribeColumnOp {
    pub id: ColumnId,
    pub description: String,
}

impl DescribeColumnOp {
    pub fn setup(id: ColumnId, description: &str) -> Self {
        Self {
            id,
            description: description.to_string(),
        }
    }
}

impl Operation for DescribeColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id).ok_or_else(|| {
            BundlebaseError::from(format!("Column with ID '{}' not found", self.id))
        })?;
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // Metadata-only; descriptions are resolved from the operation list.
        Ok(())
    }

    fn describe(&self) -> String {
        format!("DESCRIBE COLUMN: {} = {}", self.id, self.description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let id = ColumnId::generate();
        let op = DescribeColumnOp::setup(id, "the customer's total spend");
        assert_eq!(
            op.describe(),
            format!("DESCRIBE COLUMN: {} = the customer's total spend", id)
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let id = ColumnId::generate();
        let op = DescribeColumnOp::setup(id, "unit price in USD");
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let back: DescribeColumnOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, back);
    }
}
