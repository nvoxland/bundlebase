//! DropJoin command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::DropJoinOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to drop a join.
#[derive(Debug, Clone)]
pub struct DropJoinCommand {
    /// The name of the join to drop
    pub name: String,
}

impl DropJoinCommand {
    /// Create a new DropJoinCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Command for DropJoinCommand {
    fn description(&self) -> String {
        format!("Drop join '{}'", self.name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = DropJoinOp::setup(&self.name, ctx.bundle()).await?;
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }
}
