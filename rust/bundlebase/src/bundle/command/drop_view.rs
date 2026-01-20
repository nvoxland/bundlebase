//! DropView command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::DropViewOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to drop a view.
#[derive(Debug, Clone)]
pub struct DropViewCommand {
    /// The name of the view to drop
    pub name: String,
}

impl DropViewCommand {
    /// Create a new DropViewCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Command for DropViewCommand {
    fn description(&self) -> String {
        format!("Drop view '{}'", self.name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = DropViewOp::setup(&self.name, ctx.bundle()).await?;
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }
}
