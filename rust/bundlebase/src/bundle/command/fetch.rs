//! Fetch command implementations.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::{AttachBlockOp, DetachBlockOp, SourceInfo};
use crate::data::ObjectId;
use crate::source::FetchAction;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;

/// Command to fetch from sources for a specific pack.
#[derive(Debug, Clone)]
pub struct FetchCommand {
    /// The pack to fetch sources for (None or "base" for base pack)
    pub pack: Option<String>,
}

impl FetchCommand {
    /// Create a new FetchCommand.
    pub fn new(pack: Option<String>) -> Self {
        Self { pack }
    }
}

#[async_trait]
impl Command for FetchCommand {
    fn description(&self) -> String {
        let pack_name = self.pack.as_deref().unwrap_or("base");
        format!("Fetch sources for {}", pack_name)
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let pack_name = self.pack.as_deref().unwrap_or("base");
        let pack_id = match self.pack.as_deref() {
            None | Some("base") => ObjectId::BASE_PACK,
            Some(join_name) => *ctx
                .bundle()
                .pack_by_name(join_name)
                .ok_or(format!("Unknown join '{}'", join_name))?
                .id(),
        };

        let sources = ctx.bundle().get_sources_for_pack(&pack_id);
        if sources.is_empty() {
            return Err(format!("No sources defined for pack '{}'", pack_name).into());
        }

        for source in sources {
            fetch_from_source(ctx, &source, &pack_id, pack_name).await?;
        }

        Ok(())
    }
}

/// Command to fetch from all defined sources.
#[derive(Debug, Clone, Default)]
pub struct FetchAllCommand;

impl FetchAllCommand {
    /// Create a new FetchAllCommand.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Command for FetchAllCommand {
    fn description(&self) -> String {
        "Fetch all sources".to_string()
    }

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        // Collect sources with their pack info to avoid borrow issues
        let sources_with_packs: Vec<_> = ctx
            .bundle()
            .sources()
            .values()
            .map(|source| {
                let pack_name = ctx
                    .bundle()
                    .pack_name(source.pack())
                    .unwrap_or("base".to_string());
                let pack_id = *source.pack();
                (source.clone(), pack_id, pack_name)
            })
            .collect();

        for (source, pack_id, pack_name) in sources_with_packs {
            fetch_from_source(ctx, &source, &pack_id, &pack_name).await?;
        }

        Ok(())
    }
}

/// Helper to fetch from a single source.
async fn fetch_from_source(
    ctx: &mut CommandContext<'_>,
    source: &std::sync::Arc<crate::bundle::Source>,
    pack_id: &ObjectId,
    pack_name: &str,
) -> Result<(), BundlebaseError> {
    let registry = ctx.bundle().source_function_registry();
    let source_id = *source.id();

    let actions = source
        .fetch(ctx.builder().data_dir(), ctx.bundle().config(), &registry)
        .await?;

    for action in actions {
        match action {
            FetchAction::Add(data) => {
                let mut op = AttachBlockOp::setup_for_source(
                    pack_id,
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
                info!("Fetched {} to {}", data.attach_location, pack_name);
            }
            FetchAction::Replace {
                old_source_location,
                data,
            } => {
                // Find and detach the old block
                let old_location =
                    find_block_location_by_source(ctx, &source_id, &old_source_location)?;
                let detach_op = DetachBlockOp::setup(&old_location, ctx.bundle()).await?;
                ctx.apply_operation(detach_op.into()).await?;

                // Attach the new block
                let mut op = AttachBlockOp::setup_for_source(
                    pack_id,
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
                info!("Replaced {} in {}", data.attach_location, pack_name);
            }
            FetchAction::Remove { source_location } => {
                let location = find_block_location_by_source(ctx, &source_id, &source_location)?;
                let detach_op = DetachBlockOp::setup(&location, ctx.bundle()).await?;
                ctx.apply_operation(detach_op.into()).await?;
                info!("Removed {} from {}", location, pack_name);
            }
        }
    }

    Ok(())
}

/// Find the current location of a block that was attached from a source.
fn find_block_location_by_source(
    ctx: &CommandContext<'_>,
    source_id: &ObjectId,
    source_location: &str,
) -> Result<String, BundlebaseError> {
    use crate::bundle::operation::AnyOperation;

    // First, check ReplaceBlockOp operations (in reverse order to get most recent)
    for op in ctx.bundle().operations.iter().rev() {
        if let AnyOperation::ReplaceBlock(replace) = op {
            if let Some(ref info) = replace.source_info {
                if &info.id == source_id && info.location == source_location {
                    return Ok(replace.new_location.clone());
                }
            }
        }
    }

    // If not found in ReplaceBlockOp, check AttachBlockOp
    ctx.bundle()
        .operations
        .iter()
        .find_map(|op| {
            if let AnyOperation::AttachBlock(attach) = op {
                if let Some(ref info) = attach.source_info {
                    if &info.id == source_id && info.location == source_location {
                        return Some(attach.location.clone());
                    }
                }
            }
            None
        })
        .ok_or_else(|| {
            format!(
                "No block found for source_location '{}'",
                source_location
            )
            .into()
        })
}
