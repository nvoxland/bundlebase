//! RebuildIndex command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::RebuildIndexOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to rebuild an index on a column.
#[derive(Debug, Clone)]
pub struct RebuildIndexCommand {
    /// The column name to rebuild the index for
    pub column: String,
}

impl RebuildIndexCommand {
    /// Create a new RebuildIndexCommand.
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
        }
    }
}

#[async_trait]
impl Command for RebuildIndexCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = RebuildIndexOp::setup(self.column.clone()).await?;
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }

    fn to_statement(&self) -> String {
        format!("REBUILD INDEX ON {}", self.column)
    }
}
