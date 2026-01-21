//! Fetch command implementations.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::{AttachBlockOp, DetachBlockOp, SourceInfo};
use crate::data::ObjectId;
use crate::source::{FetchAction, FetchResults};
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;
use std::sync::Arc;
use super::{BuilderCommandContext, BundleBuilderCommand};

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

impl CommandParsing for FetchCommand {
    fn rule() -> Rule {
        Rule::fetch_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // Check for identifier (pack name) that is NOT "all"
        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::identifier {
                let ident = inner_pair.as_str();
                // If it's "all", this should be FetchAllCommand
                if !ident.eq_ignore_ascii_case("all") {
                    return Ok(FetchCommand::new(Some(ident.to_string())));
                }
            }
        }

        // Just "FETCH" with no pack - fetch from base pack
        Ok(FetchCommand::new(None))
    }

    fn to_statement(&self) -> String {
        match &self.pack {
            Some(pack) if pack != "base" => format!("FETCH {}", pack),
            _ => "FETCH".to_string(),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for FetchCommand {
    type Output = Vec<FetchResults>;

    async fn execute(self: Box<Self>, ctx: &mut BuilderCommandContext<'_>) -> Result<Vec<FetchResults>, BundlebaseError> {
        let pack_name = self.pack.as_deref().unwrap_or("base").to_string();
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

        let mut results = Vec::new();
        for source in sources {
            let result = fetch_from_source(ctx, &source, &pack_id, &pack_name).await?;
            results.push(result);
        }

        Ok(results)
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

impl CommandParsing for FetchAllCommand {
    fn rule() -> Rule {
        Rule::fetch_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // Check if the raw contains "ALL"
        let raw = pair.as_str().to_uppercase();
        if raw.contains("ALL") {
            return Ok(FetchAllCommand::new());
        }

        // Also check for identifier "all"
        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::identifier {
                let ident = inner_pair.as_str();
                if ident.eq_ignore_ascii_case("all") {
                    return Ok(FetchAllCommand::new());
                }
            }
        }

        Err("Expected FETCH ALL".into())
    }

    fn to_statement(&self) -> String {
        "FETCH ALL".to_string()
    }
}

#[async_trait]
impl BundleBuilderCommand for FetchAllCommand {
    type Output = Vec<FetchResults>;

    async fn execute(self: Box<Self>, ctx: &mut BuilderCommandContext<'_>) -> Result<Vec<FetchResults>, BundlebaseError> {
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

        let mut results = Vec::new();
        for (source, pack_id, pack_name) in sources_with_packs {
            let result = fetch_from_source(ctx, &source, &pack_id, &pack_name).await?;
            results.push(result);
        }

        Ok(results)
    }
}

/// Helper to fetch from a single source.
async fn fetch_from_source(
    ctx: &mut BuilderCommandContext<'_>,
    source: &Arc<crate::bundle::Source>,
    pack_id: &ObjectId,
    pack_name: &str,
) -> Result<FetchResults, BundlebaseError> {
    let registry = ctx.bundle().source_function_registry();
    let source_id = *source.id();
    let source_function = source.function().to_string();
    let source_url = source.args().get("url").cloned().unwrap_or_default();

    let actions = source
        .fetch(ctx.data_dir(), ctx.bundle().config(), &registry)
        .await?;

    // Process actions and collect them for the result
    let mut processed_actions = Vec::new();

    for action in actions {
        match &action {
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
                    location: data.source_location.clone(),
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
                    find_block_location_by_source(ctx, &source_id, old_source_location)?;
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
                    location: data.source_location.clone(),
                    version: op.version.clone(),
                });
                ctx.apply_operation(op.into()).await?;
                info!("Replaced {} in {}", data.attach_location, pack_name);
            }
            FetchAction::Remove { source_location } => {
                let location = find_block_location_by_source(ctx, &source_id, source_location)?;
                let detach_op = DetachBlockOp::setup(&location, ctx.bundle()).await?;
                ctx.apply_operation(detach_op.into()).await?;
                info!("Removed {} from {}", location, pack_name);
            }
        }
        processed_actions.push(action);
    }

    Ok(FetchResults::from_actions(
        source_function,
        source_url,
        pack_name.to_string(),
        processed_actions,
    ))
}

/// Find the current location of a block that was attached from a source.
fn find_block_location_by_source(
    ctx: &BuilderCommandContext<'_>,
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

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_fetch_base() {
        let input = "FETCH";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_pack() {
        let input = "FETCH users";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, Some("users".to_string()));
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all() {
        let input = "FETCH ALL";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(_) => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch() {
        let cmd = FetchCommand::new(None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_all() {
        let cmd = FetchAllCommand::new();
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH ALL");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::FetchAll(_) => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }
}
