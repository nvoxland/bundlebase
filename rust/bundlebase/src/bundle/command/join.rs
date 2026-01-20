//! Join command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::{AttachBlockOp, CreateJoinOp};
use crate::bundle::pack::JoinTypeOption;
use crate::data::ObjectId;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to join with another data source.
#[derive(Debug, Clone)]
pub struct JoinCommand {
    /// The name for the join
    pub name: String,
    /// The join expression
    pub expression: String,
    /// Optional location of data to attach to the join
    pub location: Option<String>,
    /// The type of join
    pub join_type: JoinTypeOption,
}

impl JoinCommand {
    /// Create a new JoinCommand.
    pub fn new(
        name: impl Into<String>,
        expression: impl Into<String>,
        location: Option<String>,
        join_type: JoinTypeOption,
    ) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
            location,
            join_type,
        }
    }
}

#[async_trait]
impl Command for JoinCommand {
    fn description(&self) -> String {
        format!("Join '{}' on {}", self.name, self.expression)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        // Step 1: Create a new pack with join metadata
        let join_pack_id = ObjectId::generate();
        ctx.apply_operation(
            CreateJoinOp::setup(&join_pack_id, &self.name, &self.expression, self.join_type)
                .await?
                .into(),
        )
        .await?;

        // Step 2: Attach the location data to the join pack (if provided)
        if let Some(ref loc) = self.location {
            let op = AttachBlockOp::setup(&join_pack_id, loc, ctx.builder()).await?;
            ctx.apply_operation(op.into()).await?;
        }

        match &self.location {
            Some(loc) => info!("Joined: {} as \"{}\"", loc, self.name),
            None => info!("Created join point \"{}\" (no initial data)", self.name),
        }

        Ok(())
    }
}
