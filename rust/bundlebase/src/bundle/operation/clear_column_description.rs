use super::Operation;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClearColumnDescriptionOp {
    pub id: ColumnId,
}

impl ClearColumnDescriptionOp {
    pub fn setup(id: ColumnId) -> Self {
        Self { id }
    }
}

impl Operation for ClearColumnDescriptionOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        super::semantic_validation::check_column(bundle, &self.id)
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!("CLEAR COLUMN DESCRIPTION: {}", self.id)
    }
}
