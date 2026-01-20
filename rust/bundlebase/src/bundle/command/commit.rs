//! Commit command implementation.
//!
//! Note: Commit is special - it doesn't use the normal Command trait because
//! it directly manipulates the builder's commit state rather than applying
//! operations within a change context.

use crate::bundle::command::{Command, CommandContext};
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to commit changes.
///
/// Note: This command is somewhat special because commit() is typically
/// called directly on BundleBuilder rather than through execute_command(),
/// since it finalizes all pending changes rather than being a tracked change itself.
#[derive(Debug, Clone)]
pub struct CommitCommand {
    /// The commit message
    pub message: String,
}

impl CommitCommand {
    /// Create a new CommitCommand.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
impl Command for CommitCommand {
    fn description(&self) -> String {
        format!("Commit: {}", self.message)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        // Commit is special - we need to call the builder's commit method directly
        // This will commit all pending changes (including any that were just added)
        ctx.builder_mut().commit(&self.message).await?;
        Ok(())
    }
}
