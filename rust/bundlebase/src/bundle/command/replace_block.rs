//! ReplaceBlock command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::command::parser::escape_string;
use crate::bundle::operation::ReplaceBlockOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to replace a block's location in the bundle.
#[derive(Debug, Clone)]
pub struct ReplaceBlockCommand {
    /// The current location (URL) of the block
    pub old_location: String,
    /// The new location (URL) to read data from
    pub new_location: String,
}

impl ReplaceBlockCommand {
    /// Create a new ReplaceBlockCommand.
    pub fn new(old_location: impl Into<String>, new_location: impl Into<String>) -> Self {
        Self {
            old_location: old_location.into(),
            new_location: new_location.into(),
        }
    }
}

#[async_trait]
impl Command for ReplaceBlockCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = ReplaceBlockOp::setup(&self.old_location, &self.new_location, ctx.builder()).await?;
        ctx.apply_operation(op.into()).await?;
        info!("Replaced block {} -> {}", self.old_location, self.new_location);
        Ok(())
    }

    fn to_statement(&self) -> String {
        format!(
            "REPLACE {} WITH {}",
            escape_string(&self.old_location),
            escape_string(&self.new_location)
        )
    }
}
