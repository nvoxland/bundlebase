//! Fetch command implementations.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::{AttachBlockOp, DetachBlockOp, SourceInfo};
use bundlebase_data::ObjectId;
use bundlebase_common::progress::ProgressScope;
use bundlebase::source::{FetchAction, FetchResults, SyncMode};
use bundlebase_common::BundlebaseError;
use log::info;
use std::sync::Arc;
use crate::BundleBuilderCommand;
use bundlebase::{Bundle, BundleBuilder};

/// Command to fetch from sources for a specific pack.
#[derive(Debug, Clone)]
pub struct FetchCommand {
    /// The pack to fetch sources for (e.g. "base", or a join name)
    pub pack: String,
    /// Sync mode for the fetch operation
    pub mode: SyncMode,
    /// If true, only compute what would change without actually executing
    pub dry_run: bool,
}

impl FetchCommand {
    /// Create a new FetchCommand.
    pub fn new(pack: String, mode: SyncMode) -> Self {
        Self { pack, mode, dry_run: false }
    }

    /// Create a new FetchCommand with dry_run flag.
    pub fn new_with_dry_run(pack: String, mode: SyncMode, dry_run: bool) -> Self {
        Self { pack, mode, dry_run }
    }
}

impl CommandParsing for FetchCommand {
    fn rule() -> Rule {
        Rule::fetch_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut pack = None;
        let mut mode = None;
        let mut dry_run = false;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::identifier => {
                    pack = Some(extract_identifier(&inner_pair));
                }
                Rule::fetch_mode => {
                    mode = Some(SyncMode::from_arg(inner_pair.as_str())?);
                }
                Rule::fetch_dry_run => {
                    dry_run = true;
                }
                _ => {}
            }
        }

        let pack = pack.ok_or_else(|| BundlebaseError::from("FETCH statement missing pack name"))?;
        let mode = mode.ok_or_else(|| BundlebaseError::from("FETCH statement missing mode"))?;

        Ok(FetchCommand::new_with_dry_run(pack, mode, dry_run))
    }

    fn to_statement(&self) -> String {
        if self.dry_run {
            format!("FETCH {} {} DRY RUN", quote_identifier(&self.pack), self.mode)
        } else {
            format!("FETCH {} {}", quote_identifier(&self.pack), self.mode)
        }
    }
}

impl BundleBuilderCommand for FetchCommand {
    type Output = Vec<FetchResults>;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<Vec<FetchResults>, BundlebaseError> {
        let pack_name = self.pack.clone();
        let pack_id = builder.resolve_pack_id(Some(&self.pack))?;

        let mode = self.mode;

        let sources = builder.bundle().get_sources_for_pack(&pack_id);
        if sources.is_empty() {
            return Err(format!("No sources defined for pack '{}'", pack_name).into());
        }

        let mut results = Vec::new();
        for source in sources {
            let result = fetch_from_source(builder, &source, &pack_id, &pack_name, mode).await?;
            results.push(result);
        }

        Ok(results)
    }
}

/// Command to fetch from all defined sources.
#[derive(Debug, Clone)]
pub struct FetchAllCommand {
    /// Sync mode for the fetch operation
    pub mode: SyncMode,
    /// If true, only compute what would change without actually executing
    pub dry_run: bool,
}

impl FetchAllCommand {
    /// Create a new FetchAllCommand.
    pub fn new(mode: SyncMode) -> Self {
        Self { mode, dry_run: false }
    }

    /// Create a new FetchAllCommand with dry_run flag.
    pub fn new_with_dry_run(mode: SyncMode, dry_run: bool) -> Self {
        Self { mode, dry_run }
    }
}

impl CommandParsing for FetchAllCommand {
    fn rule() -> Rule {
        Rule::fetch_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut mode = None;
        let mut dry_run = false;
        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::fetch_mode => {
                    mode = Some(SyncMode::from_arg(inner_pair.as_str())?);
                }
                Rule::fetch_dry_run => {
                    dry_run = true;
                }
                _ => {}
            }
        }

        let mode = mode.ok_or_else(|| BundlebaseError::from("FETCH ALL statement missing mode"))?;

        Ok(FetchAllCommand::new_with_dry_run(mode, dry_run))
    }

    fn to_statement(&self) -> String {
        if self.dry_run {
            format!("FETCH ALL {} DRY RUN", self.mode)
        } else {
            format!("FETCH ALL {}", self.mode)
        }
    }
}

impl BundleBuilderCommand for FetchAllCommand {
    type Output = Vec<FetchResults>;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<Vec<FetchResults>, BundlebaseError> {
        let mode = self.mode;

        // Collect sources with their pack info to avoid borrow issues
        let sources_with_packs: Vec<_> = builder
            .bundle()
            .sources()
            .values()
            .map(|source| {
                let pack_name = builder
                    .bundle()
                    .pack_name(source.pack())
                    .unwrap_or("base".to_string());
                let pack_id = *source.pack();
                (source.clone(), pack_id, pack_name)
            })
            .collect();

        let mut results = Vec::new();
        for (source, pack_id, pack_name) in sources_with_packs {
            let result = fetch_from_source(builder, &source, &pack_id, &pack_name, mode).await?;
            results.push(result);
        }

        Ok(results)
    }
}

/// Helper to fetch from a single source.
async fn fetch_from_source(
    builder: &BundleBuilder,
    source: &Arc<bundlebase::bundle::Source>,
    pack_id: &ObjectId,
    pack_name: &str,
    mode: SyncMode,
) -> Result<FetchResults, BundlebaseError> {
    let source_id = *source.id();
    let connector = source.connector().to_string();
    let source_url = source.args().get("url").cloned().unwrap_or_default();

    let actions = source.fetch(builder, mode).await?;

    // Process actions and collect them for the result
    let progress = ProgressScope::new(
        &format!("Applying {} fetch actions for '{}'", actions.len(), pack_name),
        Some(actions.len() as u64),
    );
    let mut processed_actions = Vec::new();

    for (idx, action) in actions.into_iter().enumerate() {
        match &action {
            FetchAction::Add(data) => {
                let temp_reader = builder.bundle().reader_factory
                    .detect(&data.attach_location, &bundlebase_data::BlockId::generate(), builder)
                    .await?;
                let format = temp_reader.format();
                let op = AttachBlockOp::setup(
                    pack_id,
                    &data.attach_location,
                    format,
                    data.hash.as_deref(),
                    Some(SourceInfo {
                        id: source_id,
                        location: data.source_location.clone(),
                        version: data.version.clone(),
                    }),
                    builder,
                )
                .await?;
                builder.apply_operation(op.into()).await?;
                info!("Fetched {} to {}", data.attach_location, pack_name);
            }
            FetchAction::Replace {
                old_source_location,
                data,
            } => {
                // Clone bundle for find_block_location_by_source lookup
                let bundle_snapshot = builder.bundle().clone();
                let old_location =
                    find_block_location_by_source(&bundle_snapshot, &source_id, old_source_location)?;
                let detach_op = DetachBlockOp::setup(&old_location, builder).await?;
                builder.apply_operation(detach_op.into()).await?;

                // Attach the new block
                let temp_reader = builder.bundle().reader_factory
                    .detect(&data.attach_location, &bundlebase_data::BlockId::generate(), builder)
                    .await?;
                let format = temp_reader.format();
                let op = AttachBlockOp::setup(
                    pack_id,
                    &data.attach_location,
                    format,
                    data.hash.as_deref(),
                    Some(SourceInfo {
                        id: source_id,
                        location: data.source_location.clone(),
                        version: data.version.clone(),
                    }),
                    builder,
                )
                .await?;
                builder.apply_operation(op.into()).await?;
                info!("Replaced {} in {}", data.attach_location, pack_name);
            }
            FetchAction::Remove { source_location } => {
                // Clone bundle for find_block_location_by_source lookup
                let bundle_snapshot = builder.bundle().clone();
                let location = find_block_location_by_source(&bundle_snapshot, &source_id, source_location)?;
                let detach_op = DetachBlockOp::setup(&location, builder).await?;
                builder.apply_operation(detach_op.into()).await?;
                info!("Removed {} from {}", location, pack_name);
            }
        }
        processed_actions.push(action);
        progress.update((idx + 1) as u64, None);
    }

    Ok(FetchResults::from_actions(
        connector,
        source_url,
        pack_name.to_string(),
        processed_actions,
    ))
}

/// Find the current location of a block that was attached from a source.
fn find_block_location_by_source(
    bundle: &Bundle,
    source_id: &ObjectId,
    source_location: &str,
) -> Result<String, BundlebaseError> {
    use bundlebase::bundle::operation::AnyOperation;

    // First, check ReplaceBlockOp operations (in reverse order to get most recent)
    let operations = bundle.operations.read();
    for op in operations.iter().rev() {
        if let AnyOperation::ReplaceBlock(replace) = op {
            if let Some(ref info) = replace.source_info {
                if &info.id == source_id && info.location == source_location {
                    return Ok(replace.new_location.clone());
                }
            }
        }
    }

    // If not found in ReplaceBlockOp, check AttachBlockOp
    operations
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
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_fetch_base() {
        let input = "FETCH base ADD";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_pack() {
        let input = "FETCH users ADD";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "users");
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_with_mode() {
        let input = "FETCH base UPDATE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Update);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_sync_mode() {
        let input = "FETCH base SYNC";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_pack_with_mode() {
        let input = "FETCH users SYNC";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "users");
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all() {
        let input = "FETCH ALL ADD";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all_with_mode() {
        let input = "FETCH ALL UPDATE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Update);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all_sync() {
        let input = "FETCH ALL SYNC";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_base() {
        let cmd = FetchCommand::new("base".to_string(), SyncMode::Add);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH base ADD");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_with_mode() {
        let cmd = FetchCommand::new("base".to_string(), SyncMode::Update);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH base UPDATE");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "base");
                assert_eq!(c.mode, SyncMode::Update);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_pack() {
        let cmd = FetchCommand::new("users".to_string(), SyncMode::Sync);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH users SYNC");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Fetch(c) => {
                assert_eq!(c.pack, "users");
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_all() {
        let cmd = FetchAllCommand::new(SyncMode::Add);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH ALL ADD");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Add);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_round_trip_fetch_all_with_mode() {
        let cmd = FetchAllCommand::new(SyncMode::Sync);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FETCH ALL SYNC");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::FetchAll(c) => {
                assert_eq!(c.mode, SyncMode::Sync);
            }
            _ => panic!("Expected FetchAll variant"),
        }
    }
}
