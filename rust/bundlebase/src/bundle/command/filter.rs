//! Filter command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::FilterOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::scalar::ScalarValue;
use log::info;

/// Command to filter rows with a WHERE clause.
#[derive(Debug, Clone)]
pub struct FilterCommand {
    /// The WHERE clause
    pub where_clause: String,
    /// Parameters for the WHERE clause ($1, $2, etc.)
    pub params: Vec<ScalarValue>,
}

impl FilterCommand {
    /// Create a new FilterCommand.
    pub fn new(where_clause: impl Into<String>, params: Vec<ScalarValue>) -> Self {
        Self {
            where_clause: where_clause.into(),
            params,
        }
    }
}

#[async_trait]
impl Command for FilterCommand {
    fn description(&self) -> String {
        format!("Filter: {}", self.where_clause)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.apply_operation(
            FilterOp::setup(&self.where_clause, self.params)
                .await?
                .into(),
        )
        .await?;
        info!("Filtered by {}", self.where_clause);
        Ok(())
    }
}
