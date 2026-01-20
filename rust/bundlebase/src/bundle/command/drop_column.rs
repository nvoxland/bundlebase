//! DropColumn command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::DropColumnOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to drop a column from the bundle.
#[derive(Debug, Clone)]
pub struct DropColumnCommand {
    /// The column name to drop
    pub name: String,
}

impl DropColumnCommand {
    /// Create a new DropColumnCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl Command for DropColumnCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.apply_operation(DropColumnOp::setup(vec![self.name.as_str()]).into())
            .await?;

        info!("Dropped column \"{}\"", self.name);

        Ok(())
    }

    fn to_statement(&self) -> String {
        format!("DROP COLUMN {}", self.name)
    }
}
