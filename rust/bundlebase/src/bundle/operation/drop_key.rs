use super::Operation;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropKeyOp {
    pub columns: Vec<ColumnId>,
    pub unique: bool,
}

impl DropKeyOp {
    pub fn setup(columns: Vec<ColumnId>, unique: bool) -> Self {
        Self { columns, unique }
    }
}

impl Operation for DropKeyOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        if !self.unique && self.columns.is_empty() {
            return Ok(());
        }
        super::semantic_validation::check_key(bundle, &self.columns)
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!(
            "DROP {} KEY: {:?}",
            if self.unique { "UNIQUE" } else { "PRIMARY" },
            self.columns
        )
    }
}
