//! CreateSource command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::{AttachBlockOp, CreateSourceOp, SourceInfo};
use crate::data::ObjectId;
use crate::source::FetchAction;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;

/// Command to create a data source for a pack.
#[derive(Debug, Clone)]
pub struct CreateSourceCommand {
    /// The source function name (e.g., "remote_dir")
    pub function: String,
    /// Function-specific arguments
    pub args: HashMap<String, String>,
    /// The pack to create the source for (None or "base" for base pack)
    pub pack: Option<String>,
}

impl CreateSourceCommand {
    /// Create a new CreateSourceCommand.
    pub fn new(
        function: impl Into<String>,
        args: HashMap<String, String>,
        pack: Option<String>,
    ) -> Self {
        Self {
            function: function.into(),
            args,
            pack,
        }
    }
}

#[async_trait]
impl Command for CreateSourceCommand {
    fn description(&self) -> String {
        let url = self
            .args
            .get("url")
            .cloned()
            .unwrap_or_else(|| "<no url>".to_string());
        let pack_name = self.pack.as_deref().unwrap_or("base");
        format!("Create source for {} at {}", pack_name, url)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let pack_name = self.pack.as_deref().unwrap_or("base").to_string();
        let pack_id = match self.pack.as_deref() {
            None | Some("base") => ObjectId::BASE_PACK,
            Some(join_name) => *ctx
                .bundle()
                .pack_by_name(join_name)
                .ok_or(format!("Unknown join '{}'", join_name))?
                .id(),
        };

        let source_id = ObjectId::generate();
        let op = CreateSourceOp::setup(source_id, pack_id, self.function.clone(), self.args.clone());

        ctx.apply_operation(op.into()).await?;

        // Automatically fetch from the newly created source
        let source = ctx
            .bundle()
            .get_source(&source_id)
            .ok_or_else(|| format!("Source '{}' not found after creation", source_id))?;

        let registry = ctx.bundle().source_function_registry();
        let actions = source
            .fetch(ctx.builder().data_dir(), ctx.bundle().config(), &registry)
            .await?;

        // Process fetch actions
        for action in actions {
            match action {
                FetchAction::Add(data) => {
                    let mut op = AttachBlockOp::setup_for_source(
                        &pack_id,
                        &data.attach_location,
                        &data.source_url,
                        &data.hash,
                        ctx.builder(),
                    )
                    .await?;
                    op.source_info = Some(SourceInfo {
                        id: source_id,
                        location: data.source_location,
                        version: op.version.clone(),
                    });
                    ctx.apply_operation(op.into()).await?;
                }
                FetchAction::Replace { .. } | FetchAction::Remove { .. } => {
                    // These shouldn't happen on initial source creation
                }
            }
        }

        Ok(())
    }
}
