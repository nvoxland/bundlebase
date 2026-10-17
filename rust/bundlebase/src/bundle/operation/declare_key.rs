use super::Operation;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeclareKeyOp {
    pub columns: Vec<ColumnId>,
    pub unique: bool,
}

impl DeclareKeyOp {
    pub fn setup(columns: Vec<ColumnId>, unique: bool) -> Self {
        Self { columns, unique }
    }
}

impl Operation for DeclareKeyOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        super::semantic_validation::check_key(bundle, &self.columns)
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!(
            "DECLARE {} KEY: {:?}",
            if self.unique { "UNIQUE" } else { "PRIMARY" },
            self.columns
        )
    }
}
