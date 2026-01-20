//! SetName command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::SetNameOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to set the bundle's name.
#[derive(Debug, Clone)]
pub struct SetNameCommand {
    /// The name to set
    pub name: String,
}

impl SetNameCommand {
    /// Create a new SetNameCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Command for SetNameCommand {
    fn description(&self) -> String {
        format!("Set name to {}", self.name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.apply_operation(SetNameOp::setup(&self.name).into())
            .await?;
        Ok(())
    }
}
