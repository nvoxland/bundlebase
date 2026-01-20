//! SetDescription command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::SetDescriptionOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to set the bundle's description.
#[derive(Debug, Clone)]
pub struct SetDescriptionCommand {
    /// The description to set
    pub description: String,
}

impl SetDescriptionCommand {
    /// Create a new SetDescriptionCommand.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
        }
    }
}

#[async_trait]
impl Command for SetDescriptionCommand {
    fn description(&self) -> String {
        format!("Set description to {}", self.description)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.apply_operation(SetDescriptionOp::setup(&self.description).into())
            .await?;
        Ok(())
    }
}
