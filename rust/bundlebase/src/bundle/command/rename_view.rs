//! RenameView command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::RenameViewOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to rename a view.
#[derive(Debug, Clone)]
pub struct RenameViewCommand {
    /// The current view name
    pub old_name: String,
    /// The new view name
    pub new_name: String,
}

impl RenameViewCommand {
    /// Create a new RenameViewCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

#[async_trait]
impl Command for RenameViewCommand {
    fn description(&self) -> String {
        format!("Rename view '{}' to '{}'", self.old_name, self.new_name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = RenameViewOp::setup(&self.old_name, &self.new_name, ctx.bundle()).await?;
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }
}
