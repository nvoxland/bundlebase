//! Fetch command implementations.

use crate::bundle::command::{CommandParsing, Rule, CommandResponse};
use crate::impl_dyn_command_response;
use crate::bundle::operation::{AttachBlockOp, DetachBlockOp, SourceInfo};
use crate::data::ObjectId;
use crate::source::{FetchAction, FetchResults};
use crate::BundlebaseError;
use arrow::array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use log::info;
use std::sync::Arc;
use super::super::BundleBuilderCommand;
use crate::bundle::{Bundle, BundleBuilder};

impl CommandResponse for Vec<FetchResults> {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("source_function", DataType::Utf8, false),
            Field::new("source_url", DataType::Utf8, false),
            Field::new("pack", DataType::Utf8, false),
            Field::new("added_count", DataType::UInt64, false),
            Field::new("replaced_count", DataType::UInt64, false),
            Field::new("removed_count", DataType::UInt64, false),
        ]))
    }

    fn output_shape() -> crate::bundle::command::response::OutputShape {
        crate::bundle::command::response::OutputShape::Table
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let source_function: ArrayRef = Arc::new(StringArray::from(
            self.iter()
                .map(|r| r.source_function.as_str())
                .collect::<Vec<_>>(),
        ));
        let source_url: ArrayRef = Arc::new(StringArray::from(
            self.iter()
                .map(|r| r.source_url.as_str())
                .collect::<Vec<_>>(),
        ));
        let pack: ArrayRef = Arc::new(StringArray::from(
            self.iter().map(|r| r.pack.as_str()).collect::<Vec<_>>(),
        ));
        let added_count: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.added.len() as u64).collect::<Vec<_>>(),
        ));
        let replaced_count: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.replaced.len() as u64).collect::<Vec<_>>(),
        ));
        let removed_count: ArrayRef = Arc::new(UInt64Array::from(
            self.iter().map(|r| r.removed.len() as u64).collect::<Vec<_>>(),
        ));

        RecordBatch::try_new(
            Self::schema(),
            vec![
                source_function,
                source_url,
                pack,
                added_count,
                replaced_count,
                removed_count,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }

    impl_dyn_command_response!(Vec<FetchResults>);
}

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

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<Vec<FetchResults>, BundlebaseError> {
        let pack_name = self.pack.as_deref().unwrap_or("base").to_string();
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;

        let sources = builder.bundle().get_sources_for_pack(&pack_id);
        if sources.is_empty() {
            return Err(format!("No sources defined for pack '{}'", pack_name).into());
        }

        let mut results = Vec::new();
        for source in sources {
            let result = fetch_from_source(builder, &source, &pack_id, &pack_name).await?;
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

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<Vec<FetchResults>, BundlebaseError> {
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
            let result = fetch_from_source(builder, &source, &pack_id, &pack_name).await?;
            results.push(result);
        }

        Ok(results)
    }
}

/// Helper to fetch from a single source.
async fn fetch_from_source(
    builder: &BundleBuilder,
    source: &Arc<crate::bundle::Source>,
    pack_id: &ObjectId,
    pack_name: &str,
) -> Result<FetchResults, BundlebaseError> {
    let source_id = *source.id();
    let source_function = source.function().to_string();
    let source_url = source.args().get("url").cloned().unwrap_or_default();

    let actions = source.fetch(builder).await?;

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
                    builder,
                )
                .await?;
                op.source_info = Some(SourceInfo {
                    id: source_id,
                    location: data.source_location.clone(),
                    version: op.version.clone(),
                });
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
                let mut op = AttachBlockOp::setup_for_source(
                    pack_id,
                    &data.attach_location,
                    &data.source_url,
                    &data.hash,
                    builder,
                )
                .await?;
                op.source_info = Some(SourceInfo {
                    id: source_id,
                    location: data.source_location.clone(),
                    version: op.version.clone(),
                });
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
    bundle: &Bundle,
    source_id: &ObjectId,
    source_location: &str,
) -> Result<String, BundlebaseError> {
    use crate::bundle::operation::AnyOperation;

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
