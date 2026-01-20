//! Select command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::SelectOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::scalar::ScalarValue;
use log::info;

/// Command to execute a SQL SELECT query.
#[derive(Debug, Clone)]
pub struct SelectCommand {
    /// The SQL query
    pub sql: String,
    /// Parameters for the query ($1, $2, etc.)
    pub params: Vec<ScalarValue>,
}

impl SelectCommand {
    /// Create a new SelectCommand.
    pub fn new(sql: impl Into<String>, params: Vec<ScalarValue>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

#[async_trait]
impl Command for SelectCommand {
    fn description(&self) -> String {
        format!("Query: {}", self.sql)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let sql = if !self.sql.to_lowercase().starts_with("select ") {
            format!("SELECT {}", self.sql)
        } else {
            self.sql
        };

        ctx.apply_operation(SelectOp::setup(sql, self.params).await?.into())
            .await?;
        info!("Created query");
        Ok(())
    }
}
