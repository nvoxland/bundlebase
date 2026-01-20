//! CreateIndex command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::CreateIndexOp;
use crate::index::IndexType;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to create an index on a column.
#[derive(Debug, Clone)]
pub struct CreateIndexCommand {
    /// The column to index
    pub column: String,
    /// The type of index to create
    pub index_type: IndexType,
}

impl CreateIndexCommand {
    /// Create a new CreateIndexCommand.
    pub fn new(column: impl Into<String>, index_type: IndexType) -> Self {
        Self {
            column: column.into(),
            index_type,
        }
    }
}

#[async_trait]
impl Command for CreateIndexCommand {
    fn description(&self) -> String {
        match &self.index_type {
            IndexType::Column => format!("Index column {}", self.column),
            IndexType::Text { tokenizer } => {
                format!("Text index column {} (tokenizer: {:?})", self.column, tokenizer)
            }
        }
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.apply_operation(
            CreateIndexOp::setup(&self.column, self.index_type.clone())
                .await?
                .into(),
        )
        .await?;

        ctx.reindex_internal().await?;

        info!("Created index on: \"{}\"", self.column);

        Ok(())
    }
}
