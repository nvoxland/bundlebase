//! RenameColumn command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::RenameColumnOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to rename a column.
#[derive(Debug, Clone)]
pub struct RenameColumnCommand {
    /// The current column name
    pub old_name: String,
    /// The new column name
    pub new_name: String,
}

impl RenameColumnCommand {
    /// Create a new RenameColumnCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

#[async_trait]
impl Command for RenameColumnCommand {
    fn description(&self) -> String {
        format!("Rename column '{}' to '{}'", self.old_name, self.new_name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.apply_operation(RenameColumnOp::setup(&self.old_name, &self.new_name).into())
            .await?;
        info!("Renamed \"{}\" to \"{}\"", self.old_name, self.new_name);
        Ok(())
    }
}
