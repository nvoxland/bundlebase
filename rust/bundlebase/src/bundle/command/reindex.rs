//! Reindex command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to rebuild all indexes.
#[derive(Debug, Clone, Default)]
pub struct ReindexCommand;

impl ReindexCommand {
    /// Create a new ReindexCommand.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Command for ReindexCommand {
    fn description(&self) -> String {
        "Reindex".to_string()
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.reindex_internal().await
    }
}
