//! Attach command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::AttachBlockOp;
use crate::data::ObjectId;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to attach a data block to the bundle.
#[derive(Debug, Clone)]
pub struct AttachCommand {
    /// The path/URL of the data to attach
    pub path: String,
    /// The pack to attach to (None or "base" for base pack, otherwise join name)
    pub pack: Option<String>,
}

impl AttachCommand {
    /// Create a new AttachCommand.
    pub fn new(path: impl Into<String>, pack: Option<String>) -> Self {
        Self {
            path: path.into(),
            pack,
        }
    }
}

#[async_trait]
impl Command for AttachCommand {
    fn description(&self) -> String {
        let pack_name = self.pack.as_deref().unwrap_or("base");
        format!("Attach {} to {}", self.path, pack_name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let pack_id = match self.pack.as_deref() {
            None | Some("base") => ObjectId::BASE_PACK,
            Some(join_name) => *ctx
                .bundle()
                .pack_by_name(join_name)
                .ok_or(format!("Unknown join '{}'", join_name))?
                .id(),
        };

        let pack_name = self.pack.as_deref().unwrap_or("base");

        let op = AttachBlockOp::setup(&pack_id, &self.path, ctx.builder()).await?;
        ctx.apply_operation(op.into()).await?;

        info!("Attached {} to {}", self.path, pack_name);

        Ok(())
    }
}
