//! ImportJoin command implementation.
//!
//! Solidifies an existing `bundle://` join by copying all commits, data files,
//! indexes, and connectors/functions from the source bundle into the target.
//! Operations referencing the source base pack are remapped to the join pack.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::*;
use bundlebase_data::ObjectId;
use bundlebase::{Bundle, BundleBuilder, BundleFacade};
use bundlebase_common::BundlebaseError;
use std::collections::HashMap;
use tracing::info;

use crate::BundleBuilderCommand;

/// Command to import (solidify) an existing bundle:// join.
///
/// Looks up an existing join pack by name, finds the `bundle://` source URL
/// from its attach operations, opens that source bundle, and copies all its
/// data, commits, and indexes into the target bundle.
#[derive(Debug, Clone)]
pub struct ImportJoinCommand {
    /// Name of the existing join pack to import
    pub name: String,
    /// Whether to flatten all imported commits into one
    pub flatten: bool,
}

impl ImportJoinCommand {
    pub fn new(name: impl Into<String>, flatten: bool) -> Self {
        Self {
            name: name.into(),
            flatten,
        }
    }
}

impl CommandParsing for ImportJoinCommand {
    fn rule() -> Rule {
        Rule::import_join_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut flatten = false;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    name = Some(extract_identifier(&inner));
                }
                Rule::import_join_flatten => {
                    flatten = true;
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| BundlebaseError::from("IMPORT JOIN: missing join name"))?;
        Ok(ImportJoinCommand::new(name, flatten))
    }

    fn to_statement(&self) -> String {
        if self.flatten {
            format!("IMPORT JOIN {} FLATTEN HISTORY", quote_identifier(&self.name))
        } else {
            format!("IMPORT JOIN {}", quote_identifier(&self.name))
        }
    }
}

/// Find the bundle:// source URL for a join pack by scanning attach operations.
fn find_bundle_source_url(
    builder: &BundleBuilder,
    pack_id: &ObjectId,
) -> Result<String, BundlebaseError> {
    let operations = builder.bundle().operations();
    for op in &operations {
        if let AnyOperation::AttachBlock(attach) = op {
            if attach.pack == *pack_id
                && (attach.location.starts_with("bundle://")
                    || attach.location.starts_with("bundlebase://")
                    || attach.location.starts_with("bundle+")
                    || attach.location.starts_with("bundlebase+"))
            {
                // Extract the bundle path from the URL
                let path = if attach.location.starts_with("bundle+") {
                    &attach.location["bundle+".len()..]
                } else if attach.location.starts_with("bundlebase+") {
                    &attach.location["bundlebase+".len()..]
                } else if attach.location.starts_with("bundle://") {
                    &attach.location["bundle://".len()..]
                } else {
                    &attach.location["bundlebase://".len()..]
                };
                return Ok(path.to_string());
            }
        }
    }

    Err(format!(
        "Join pack has no bundle:// data source. IMPORT JOIN only works on joins created with 'JOIN bundle://...'."
    ).into())
}

/// Remap an operation from the source bundle context to the target bundle context.
///
/// - Base pack references become the target join pack ID
/// - Source join pack IDs are remapped via the provided mapping
/// - Data file locations are updated for copied files
/// - Bundle-level metadata ops (SetName, SetDescription, etc.) are stripped
fn remap_operation(
    op: &AnyOperation,
    pack_remap: &HashMap<ObjectId, ObjectId>,
    location_remap: &HashMap<String, String>,
) -> Option<AnyOperation> {
    match op {
        // --- Remap pack field ---
        AnyOperation::AttachBlock(attach) => {
            let new_pack = pack_remap.get(&attach.pack).copied().unwrap_or(attach.pack);
            let new_location = location_remap
                .get(&attach.location)
                .cloned()
                .unwrap_or_else(|| attach.location.clone());
            Some(AnyOperation::AttachBlock(AttachBlockOp {
                pack: new_pack,
                location: new_location,
                ..attach.clone()
            }))
        }
        AnyOperation::CreateSource(source) => {
            let new_pack = pack_remap.get(&source.pack).copied().unwrap_or(source.pack);
            Some(AnyOperation::CreateSource(CreateSourceOp {
                pack: new_pack,
                ..source.clone()
            }))
        }

        // --- Remap join pack IDs ---
        AnyOperation::CreateJoin(join) => {
            let new_id = pack_remap.get(&join.id).copied().unwrap_or(join.id);
            Some(AnyOperation::CreateJoin(CreateJoinOp {
                id: new_id,
                ..join.clone()
            }))
        }
        AnyOperation::DropJoin(drop) => {
            let new_id = pack_remap.get(&drop.id).copied().unwrap_or(drop.id);
            Some(AnyOperation::DropJoin(DropJoinOp { id: new_id }))
        }
        AnyOperation::RenameJoin(rename) => {
            let new_id = pack_remap.get(&rename.id).copied().unwrap_or(rename.id);
            Some(AnyOperation::RenameJoin(RenameJoinOp {
                id: new_id,
                ..rename.clone()
            }))
        }

        // --- Remap location in replace ---
        AnyOperation::ReplaceBlock(replace) => {
            let new_location = location_remap
                .get(&replace.new_location)
                .cloned()
                .unwrap_or_else(|| replace.new_location.clone());
            Some(AnyOperation::ReplaceBlock(ReplaceBlockOp {
                new_location,
                ..replace.clone()
            }))
        }

        // --- Remap index path ---
        AnyOperation::IndexBlocks(idx) => {
            let new_path = location_remap
                .get(&idx.path)
                .cloned()
                .unwrap_or_else(|| idx.path.clone());
            Some(AnyOperation::IndexBlocks(IndexBlocksOp {
                path: new_path,
                ..idx.clone()
            }))
        }

        // --- Strip bundle-level metadata ---
        AnyOperation::SetName(_)
        | AnyOperation::SetDescription(_)
        | AnyOperation::SaveConfig(_)
        | AnyOperation::CreateView(_)
        | AnyOperation::DropView(_)
        | AnyOperation::RenameView(_) => None,

        // --- Keep everything else as-is ---
        _ => Some(op.clone()),
    }
}

/// Build the pack ID remap table from source bundle to target join pack.
fn build_pack_remap(
    source: &Bundle,
    target_join_pack_id: ObjectId,
) -> HashMap<ObjectId, ObjectId> {
    let mut remap = HashMap::new();

    // Source base pack → existing target join pack
    remap.insert(ObjectId::BASE_PACK, target_join_pack_id);

    // Source join packs → new unique IDs (avoid collisions in target)
    for (pack_id, _pack) in source.packs().iter() {
        if *pack_id != ObjectId::BASE_PACK {
            remap.insert(*pack_id, ObjectId::generate());
        }
    }

    remap
}

impl BundleBuilderCommand for ImportJoinCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        builder: &BundleBuilder,
    ) -> Result<String, BundlebaseError> {
        info!("Importing join '{}'", self.name);

        // 1. Look up existing join pack
        let target_bundle = builder.bundle();
        let pack = target_bundle.pack_by_name(&self.name).ok_or_else(|| {
            BundlebaseError::from(format!(
                "Join '{}' not found. Available joins: {}",
                self.name,
                target_bundle.join_names().join(", ")
            ))
        })?;
        let pack_id = *pack.id();

        // 2. Find the bundle:// source URL from operations
        let source_path = find_bundle_source_url(builder, &pack_id)?;

        info!("Source bundle for '{}': {}", self.name, source_path);

        // 3. Open source bundle
        let source = Bundle::open(&source_path, None).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to open source bundle '{}': {}",
                source_path, e
            ))
        })?;

        // 4. Check for function/connector name conflicts
        {
            let target_reg_arc = target_bundle.connector_registry();
            let target_registry = target_reg_arc.read();
            let source_reg_arc = source.connector_registry();
            let source_registry = source_reg_arc.read();
            for source_entry in source_registry.entries() {
                if target_registry.has_entry(&source_entry.name.to_string()) {
                    return Err(format!(
                        "Cannot import: connector '{}' already exists in target bundle. Rename it in the source bundle first.",
                        source_entry.name
                    ).into());
                }
            }
        }
        {
            let source_reg_arc = source.function_registry();
            let source_registry = source_reg_arc.read();
            for source_entry in source_registry.entries() {
                let name = source_entry.name.to_string();
                let target_reg_arc = target_bundle.function_registry();
                let target_registry = target_reg_arc.read();
                let conflict = target_registry.entries().iter().any(|e| e.name.to_string() == name);
                if conflict {
                    return Err(format!(
                        "Cannot import: function '{}' already exists in target bundle. Rename it in the source bundle first.",
                        name
                    ).into());
                }
            }
        }

        // 5. Build pack remap (source base → this join pack, source joins → new IDs)
        let pack_remap = build_pack_remap(&source, pack_id);

        // 6. Copy data files from source to target
        let location_remap = copy_data_files(&source, builder).await?;

        // 7. Get source commits and transform + replay operations
        let source_commits = source.history();

        if self.flatten {
            for commit in &source_commits {
                for change in &commit.changes {
                    for op in &change.operations {
                        if let Some(remapped) = remap_operation(op, &pack_remap, &location_remap) {
                            builder.apply_operation(remapped).await?;
                        }
                    }
                }
            }
        } else {
            for commit in &source_commits {
                let mut has_ops = false;
                for change in &commit.changes {
                    for op in &change.operations {
                        if let Some(remapped) = remap_operation(op, &pack_remap, &location_remap) {
                            builder.apply_operation(remapped).await?;
                            has_ops = true;
                        }
                    }
                }

                if has_ops {
                    let message = format!("[import {}] {}", self.name, commit.message);
                    builder.commit(&message).await?;
                }
            }
        }

        let commit_count = source_commits.len();
        Ok(format!(
            "Imported join '{}' ({} commit{})",
            self.name,
            commit_count,
            if commit_count == 1 { "" } else { "s" }
        ))
    }
}

/// Copy data files from source bundle's data_dir to target bundle's data_dir.
///
/// Returns a mapping of old locations to new locations for operation remapping.
async fn copy_data_files(
    source: &Bundle,
    target: &BundleBuilder,
) -> Result<HashMap<String, String>, BundlebaseError> {
    let source_dir = source.data_dir();
    let target_dir = target.bundle().data_dir();
    let mut location_remap = HashMap::new();

    let source_base_url = source_dir.url().to_string();
    let target_base_url = target_dir.url().to_string();

    let files = source_dir.list_files().await?;
    for file_info in files {
        let file_url = file_info.url.to_string();

        // Skip manifest directory
        if file_url.contains("_bundlebase/") || file_url.contains("_bundlebase%2F") {
            continue;
        }

        // Compute relative path from source base URL
        let relative = if let Some(rel) = file_url.strip_prefix(source_base_url.trim_end_matches('/')) {
            rel.trim_start_matches('/')
        } else {
            continue;
        };

        if relative.is_empty() {
            continue;
        }

        let source_file = source_dir.file(relative)?;
        let target_file = target_dir.writable_file(relative)?;

        let stream = source_file.read_stream().await?.ok_or_else(|| {
            BundlebaseError::from(format!(
                "Failed to read file during import: {}",
                relative
            ))
        })?;
        let pinned: std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send>> =
            Box::pin(futures::StreamExt::map(stream, |r| {
                r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            }));
        target_file.write_stream(pinned).await?;

        let new_url = format!("{}/{}", target_base_url.trim_end_matches('/'), relative);
        if file_url != new_url {
            location_remap.insert(file_url, new_url);
        }
    }

    Ok(location_remap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    // --- Parsing tests ---

    #[test]
    fn test_parse_import_join_basic() {
        let cmd = parse_command("IMPORT JOIN stations")
            .expect("Failed to parse IMPORT JOIN");
        match cmd {
            BundleCommand::ImportJoin(ref c) => {
                assert_eq!(c.name, "stations");
                assert!(!c.flatten);
            }
            _ => panic!("Expected ImportJoin variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_import_join_flatten() {
        let cmd = parse_command("IMPORT JOIN stations FLATTEN HISTORY")
            .expect("Failed to parse IMPORT JOIN FLATTEN");
        match cmd {
            BundleCommand::ImportJoin(ref c) => {
                assert_eq!(c.name, "stations");
                assert!(c.flatten);
            }
            _ => panic!("Expected ImportJoin variant"),
        }
    }

    #[test]
    fn test_parse_import_join_case_insensitive() {
        let cmd = parse_command("import join orders")
            .expect("Failed to parse case-insensitive IMPORT JOIN");
        match cmd {
            BundleCommand::ImportJoin(ref c) => {
                assert_eq!(c.name, "orders");
            }
            _ => panic!("Expected ImportJoin variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ImportJoinCommand::new("stations", false);
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT JOIN stations");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ImportJoin(ref c) => {
                assert_eq!(c.name, "stations");
                assert!(!c.flatten);
            }
            _ => panic!("Expected ImportJoin variant"),
        }
    }

    #[test]
    fn test_round_trip_flatten() {
        let cmd = ImportJoinCommand::new("orders", true);
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT JOIN orders FLATTEN HISTORY");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ImportJoin(ref c) => {
                assert!(c.flatten);
            }
            _ => panic!("Expected ImportJoin variant"),
        }
    }

    // --- Remap tests (unchanged from import_bundle) ---

    #[test]
    fn test_remap_attach_block_op() {
        let mut pack_remap = HashMap::new();
        let new_join_id = ObjectId::generate();
        pack_remap.insert(ObjectId::BASE_PACK, new_join_id);

        let op = AnyOperation::AttachBlock(AttachBlockOp {
            id: "00000000000000cc".try_into().unwrap(),
            pack: ObjectId::BASE_PACK,
            location: "old://data.csv".to_string(),
            read_options: None,
            version: "1".to_string(),
            hash: "0".repeat(64),
            source_info: None,
            layout: None,
            num_rows: Some(100),
            bytes: None,
            schema: None,
            column_ids: vec![],
        });

        let mut loc_remap = HashMap::new();
        loc_remap.insert("old://data.csv".to_string(), "new://data.csv".to_string());

        let remapped = remap_operation(&op, &pack_remap, &loc_remap).expect("should not be stripped");
        match remapped {
            AnyOperation::AttachBlock(a) => {
                assert_eq!(a.pack, new_join_id);
                assert_eq!(a.location, "new://data.csv");
            }
            _ => panic!("Expected AttachBlock"),
        }
    }

    #[test]
    fn test_remap_strips_set_name() {
        let op = AnyOperation::SetName(SetNameOp { name: "old".to_string() });
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_none());
    }

    #[test]
    fn test_remap_strips_set_description() {
        let op = AnyOperation::SetDescription(SetDescriptionOp { description: "old".to_string() });
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_none());
    }

    #[test]
    fn test_remap_keeps_filter() {
        let op = AnyOperation::Filter(FilterOp::new("SELECT * FROM bundle WHERE active", vec![]));
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_some());
    }

    #[test]
    fn test_remap_create_source_pack() {
        let mut pack_remap = HashMap::new();
        let new_join_id = ObjectId::generate();
        pack_remap.insert(ObjectId::BASE_PACK, new_join_id);

        let op = AnyOperation::CreateSource(CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::BASE_PACK,
            connector: "http".to_string(),
            args: HashMap::new(),
            save_as: None,
        });

        let remapped = remap_operation(&op, &pack_remap, &HashMap::new()).unwrap();
        match remapped {
            AnyOperation::CreateSource(s) => assert_eq!(s.pack, new_join_id),
            _ => panic!("Expected CreateSource"),
        }
    }

    #[test]
    fn test_remap_create_join_id() {
        let mut pack_remap = HashMap::new();
        let old_id: ObjectId = "00000000000000a5".try_into().unwrap();
        let new_id = ObjectId::generate();
        pack_remap.insert(old_id, new_id);

        let op = AnyOperation::CreateJoin(CreateJoinOp {
            id: old_id,
            name: "sub".to_string(),
            join_type: bundlebase::bundle::JoinTypeOption::Left,
            expression: "a = b".to_string(),
        });

        match remap_operation(&op, &pack_remap, &HashMap::new()).unwrap() {
            AnyOperation::CreateJoin(j) => assert_eq!(j.id, new_id),
            _ => panic!("Expected CreateJoin"),
        }
    }

    #[test]
    fn test_remap_drop_join_id() {
        let mut pack_remap = HashMap::new();
        let old_id: ObjectId = "00000000000000a5".try_into().unwrap();
        let new_id = ObjectId::generate();
        pack_remap.insert(old_id, new_id);

        match remap_operation(&AnyOperation::DropJoin(DropJoinOp { id: old_id }), &pack_remap, &HashMap::new()).unwrap() {
            AnyOperation::DropJoin(d) => assert_eq!(d.id, new_id),
            _ => panic!("Expected DropJoin"),
        }
    }

    #[test]
    fn test_remap_rename_join_id() {
        let mut pack_remap = HashMap::new();
        let old_id: ObjectId = "00000000000000a5".try_into().unwrap();
        let new_id = ObjectId::generate();
        pack_remap.insert(old_id, new_id);

        match remap_operation(&AnyOperation::RenameJoin(RenameJoinOp { id: old_id, new_name: "x".into() }), &pack_remap, &HashMap::new()).unwrap() {
            AnyOperation::RenameJoin(r) => assert_eq!(r.id, new_id),
            _ => panic!("Expected RenameJoin"),
        }
    }

    #[test]
    fn test_remap_replace_block_location() {
        let mut loc = HashMap::new();
        loc.insert("old://f.csv".to_string(), "new://f.csv".to_string());

        match remap_operation(&AnyOperation::ReplaceBlock(ReplaceBlockOp {
            id: "00000000000000cc".try_into().unwrap(),
            new_location: "old://f.csv".into(), new_version: "2".into(), new_hash: "0".repeat(64), source_info: None,
        }), &HashMap::new(), &loc).unwrap() {
            AnyOperation::ReplaceBlock(r) => assert_eq!(r.new_location, "new://f.csv"),
            _ => panic!("Expected ReplaceBlock"),
        }
    }

    #[test]
    fn test_remap_index_blocks_path() {
        let mut loc = HashMap::new();
        loc.insert("old://idx".to_string(), "new://idx".to_string());

        match remap_operation(&AnyOperation::IndexBlocks(IndexBlocksOp {
            index: ObjectId::generate(), blocks: vec![], path: "old://idx".into(), cardinality: 10, doc_count: None,
        }), &HashMap::new(), &loc).unwrap() {
            AnyOperation::IndexBlocks(i) => assert_eq!(i.path, "new://idx"),
            _ => panic!("Expected IndexBlocks"),
        }
    }

    #[test]
    fn test_remap_strips_save_config() {
        let yaml = "type: saveConfig\nscope: s3\nkey: region\nvalue: us-east-1";
        let op: AnyOperation = serde_yaml_ng::from_str(yaml).expect("deserialize");
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_none());
    }

    #[test]
    fn test_remap_strips_create_view() {
        let op = AnyOperation::CreateView(CreateViewOp { id: ObjectId::generate(), name: "v".into() });
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_none());
    }

    #[test]
    fn test_remap_keeps_column_ops() {
        assert!(remap_operation(&AnyOperation::RenameColumn(RenameColumnOp {
            id: "00000000000000c1".try_into().unwrap(), new_name: "x".into(),
        }), &HashMap::new(), &HashMap::new()).is_some());

        assert!(remap_operation(&AnyOperation::DropColumn(DropColumnOp {
            id: "00000000000000c1".try_into().unwrap(),
        }), &HashMap::new(), &HashMap::new()).is_some());

        assert!(remap_operation(&AnyOperation::DetachBlock(DetachBlockOp {
            id: "00000000000000cc".try_into().unwrap(),
        }), &HashMap::new(), &HashMap::new()).is_some());
    }
}
