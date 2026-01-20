//! Undo command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to undo the last uncommitted change.
#[derive(Debug, Clone, Default)]
pub struct UndoCommand;

impl UndoCommand {
    /// Create a new UndoCommand.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Command for UndoCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        // Undo is special - we call the builder's undo method directly
        ctx.builder_mut().undo().await?;
        Ok(())
    }

    fn to_statement(&self) -> String {
        "UNDO".to_string()
    }
}
