//! Reset command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to reset all uncommitted changes.
#[derive(Debug, Clone, Default)]
pub struct ResetCommand;

impl ResetCommand {
    /// Create a new ResetCommand.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Command for ResetCommand {
    fn description(&self) -> String {
        "Reset".to_string()
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        // Reset is special - we call the builder's reset method directly
        ctx.builder_mut().reset().await?;
        Ok(())
    }
}
