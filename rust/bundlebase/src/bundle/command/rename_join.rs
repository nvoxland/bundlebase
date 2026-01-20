//! RenameJoin command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::RenameJoinOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to rename a join.
#[derive(Debug, Clone)]
pub struct RenameJoinCommand {
    /// The current join name
    pub old_name: String,
    /// The new join name
    pub new_name: String,
}

impl RenameJoinCommand {
    /// Create a new RenameJoinCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

#[async_trait]
impl Command for RenameJoinCommand {
    fn description(&self) -> String {
        format!("Rename join '{}' to '{}'", self.old_name, self.new_name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = RenameJoinOp::setup(&self.old_name, &self.new_name, ctx.bundle()).await?;
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }
}
