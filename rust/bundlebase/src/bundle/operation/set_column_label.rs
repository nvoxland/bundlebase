use super::Operation;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetColumnLabelOp {
    pub id: ColumnId,
    pub label: Option<String>,
}

impl SetColumnLabelOp {
    pub fn setup(id: ColumnId, label: Option<&str>) -> Self {
        Self {
            id,
            label: label.map(str::to_string),
        }
    }
}

impl Operation for SetColumnLabelOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        super::semantic_validation::check_column(bundle, &self.id)
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET COLUMN LABEL: {} = {:?}", self.id, self.label)
    }
}
