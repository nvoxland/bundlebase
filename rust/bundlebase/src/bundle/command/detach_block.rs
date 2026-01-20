//! DetachBlock command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::command::parser::escape_string;
use crate::bundle::operation::DetachBlockOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to detach a data block from the bundle by its location.
#[derive(Debug, Clone)]
pub struct DetachBlockCommand {
    /// The location (URL) of the block to detach
    pub location: String,
}

impl DetachBlockCommand {
    /// Create a new DetachBlockCommand.
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
        }
    }
}

#[async_trait]
impl Command for DetachBlockCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = DetachBlockOp::setup(&self.location, ctx.bundle()).await?;
        ctx.apply_operation(op.into()).await?;
        info!("Detached block from {}", self.location);
        Ok(())
    }

    fn to_statement(&self) -> String {
        format!("DETACH {}", escape_string(&self.location))
    }
}
