//! CreateView command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::CreateViewOp;
use crate::BundleBuilder;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to create a view from a BundleBuilder.
///
/// Note: This command is somewhat special because it needs a reference to
/// another BundleBuilder (the source) which contains the operations to capture.
#[derive(Clone)]
pub struct CreateViewCommand {
    /// The name for the view
    pub name: String,
    /// The source builder containing operations to capture (cloned)
    pub source: BundleBuilder,
}

impl std::fmt::Debug for CreateViewCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateViewCommand")
            .field("name", &self.name)
            .field("source", &"<BundleBuilder>")
            .finish()
    }
}

impl CreateViewCommand {
    /// Create a new CreateViewCommand.
    pub fn new(name: impl Into<String>, source: &BundleBuilder) -> Self {
        Self {
            name: name.into(),
            source: source.clone(),
        }
    }
}

#[async_trait]
impl Command for CreateViewCommand {
    fn description(&self) -> String {
        format!("Create view '{}'", self.name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = CreateViewOp::setup(&self.name, &self.source, ctx.builder()).await?;
        ctx.apply_operation(op.into()).await?;
        info!("Attached view '{}'", self.name);
        Ok(())
    }
}
