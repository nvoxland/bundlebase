//! DropIndex command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::DropIndexOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to drop an index on a column.
#[derive(Debug, Clone)]
pub struct DropIndexCommand {
    /// The column whose index should be dropped
    pub column: String,
}

impl DropIndexCommand {
    /// Create a new DropIndexCommand.
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
        }
    }
}

#[async_trait]
impl Command for DropIndexCommand {
    fn description(&self) -> String {
        format!("Drop index on column {}", self.column)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        // Find the index ID for the given column
        let index_id = {
            let indexes = ctx.bundle().indexes().read();
            let index = indexes
                .iter()
                .find(|idx| idx.column() == self.column.as_str());

            match index {
                Some(idx) => *idx.id(),
                None => {
                    return Err(format!("No index found for column '{}'", self.column).into());
                }
            }
        };

        ctx.apply_operation(DropIndexOp::setup(&index_id).await?.into())
            .await?;

        info!("Dropped index on: \"{}\"", self.column);

        Ok(())
    }
}
