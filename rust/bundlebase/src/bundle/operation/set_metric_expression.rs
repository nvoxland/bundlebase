use super::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetMetricExpressionOp {
    pub name: String,
    pub expression: String,
}

impl SetMetricExpressionOp {
    pub fn setup(name: &str, expression: &str) -> Self {
        Self {
            name: name.to_string(),
            expression: expression.to_string(),
        }
    }
}

impl Operation for SetMetricExpressionOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        super::semantic_validation::check_metric(bundle, &self.name)?;
        super::semantic_validation::check_expression(&self.name, &self.expression)?;
        Box::pin(
            super::DefineMetricOp::setup(&self.name, &self.expression).validate_expression(bundle),
        )
        .await
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET METRIC EXPRESSION: {} = {}", self.name, self.expression)
    }
}
