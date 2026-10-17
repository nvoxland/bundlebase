use super::Operation;
use super::SemanticTarget;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetAiContextOp {
    pub target: SemanticTarget,
    pub context: Option<Value>,
}

impl SetAiContextOp {
    pub fn setup(target: SemanticTarget, context: Option<Value>) -> Self {
        Self { target, context }
    }
}

impl Operation for SetAiContextOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        self.target.check(bundle)?;
        if let Some(context) = &self.context {
            crate::bundle::semantic_state::validate_ai_context(context)?;
        }
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET AI CONTEXT: {:?} = {:?}", self.target, self.context)
    }
}
