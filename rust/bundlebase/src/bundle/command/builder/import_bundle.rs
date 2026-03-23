//! ImportBundle command implementation.
//!
//! Imports a bundle as a join pack in the target bundle, copying all commits,
//! data files, indexes, and connectors/functions. Operations referencing the
//! source base pack are remapped to the new join pack.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::commit::BundleCommit;
use crate::bundle::operation::*;
use crate::bundle::pack::JoinTypeOption;
use crate::data::ObjectId;
use crate::{Bundle, BundleBuilder, BundleFacade, BundlebaseError};
use async_trait::async_trait;
use std::collections::HashMap;
use tracing::info;

use super::super::BundleBuilderCommand;

/// Command to import a bundle as a join pack.
#[derive(Debug, Clone)]
pub struct ImportBundleCommand {
    /// Path to the source bundle
    pub path: String,
    /// Name for the new join pack
    pub name: String,
    /// Join expression
    pub expression: String,
    /// Whether to flatten all imported commits into one
    pub flatten: bool,
}

impl ImportBundleCommand {
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        expression: impl Into<String>,
        flatten: bool,
    ) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            expression: expression.into(),
            flatten,
        }
    }
}

impl CommandParsing for ImportBundleCommand {
    fn rule() -> Rule {
        Rule::import_bundle_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut name = None;
        let mut expression = None;
        let mut flatten = false;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::quoted_string => {
                    path = Some(extract_string_content(inner.as_str())?);
                }
                Rule::import_bundle_flatten => {
                    flatten = true;
                }
                Rule::identifier => {
                    name = Some(inner.as_str().to_string());
                }
                Rule::import_bundle_expression => {
                    expression = Some(inner.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        let path = path.ok_or_else(|| BundlebaseError::from("IMPORT BUNDLE: missing path"))?;
        let name = name.ok_or_else(|| BundlebaseError::from("IMPORT BUNDLE: missing AS name"))?;
        let expression =
            expression.ok_or_else(|| BundlebaseError::from("IMPORT BUNDLE: missing ON expression"))?;

        Ok(ImportBundleCommand::new(path, name, expression, flatten))
    }

    fn to_statement(&self) -> String {
        if self.flatten {
            format!(
                "IMPORT BUNDLE '{}' FLATTEN HISTORY AS {} ON {}",
                self.path, self.name, self.expression
            )
        } else {
            format!(
                "IMPORT BUNDLE '{}' AS {} ON {}",
                self.path, self.name, self.expression
            )
        }
    }
}

/// Remap an operation from the source bundle context to the target bundle context.
///
/// - Base pack references become the new join pack ID
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

/// Build the pack ID remap table from source bundle to target.
fn build_pack_remap(
    source: &Bundle,
    new_join_pack_id: ObjectId,
) -> HashMap<ObjectId, ObjectId> {
    let mut remap = HashMap::new();

    // Source base pack → new join pack
    remap.insert(ObjectId::BASE_PACK, new_join_pack_id);

    // Source join packs → new unique IDs
    for (pack_id, _pack) in source.packs().read().iter() {
        if *pack_id != ObjectId::BASE_PACK {
            remap.insert(*pack_id, ObjectId::generate());
        }
    }

    remap
}

#[async_trait]
impl BundleBuilderCommand for ImportBundleCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        builder: &BundleBuilder,
    ) -> Result<String, BundlebaseError> {
        info!("Importing bundle from '{}' as '{}'", self.path, self.name);

        // 1. Open source bundle
        let source = Bundle::open(&self.path, None).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to open source bundle '{}': {}",
                self.path, e
            ))
        })?;

        // 2. Check for function/connector name conflicts
        let target_bundle = builder.bundle();
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

        // 3. Generate new join pack ID and build remap table
        let join_pack_id = ObjectId::generate();
        let pack_remap = build_pack_remap(&source, join_pack_id);

        // 4. Copy data files from source to target
        let location_remap = copy_data_files(&source, builder).await?;

        // 5. Create the join pack operation
        let create_join = AnyOperation::CreateJoin(CreateJoinOp {
            id: join_pack_id,
            name: self.name.clone(),
            join_type: JoinTypeOption::Inner,
            expression: self.expression.clone(),
        });

        // 6. Get source commits and transform operations
        let source_commits = source.history();

        if self.flatten {
            // Flatten: all operations into one change, caller will commit
            builder.apply_operation(create_join).await?;

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
            // Separate commits: first commit creates the join pack
            builder.apply_operation(create_join).await?;
            builder.commit(&format!("[import {}] Create join pack '{}'", self.name, self.name)).await?;

            // Then replay each source commit
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
            "Imported bundle '{}' as '{}' ({} commit{})",
            self.path,
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
    use crate::io::{IOReadWriteDir, IOReadWriteFile};

    let source_dir = source.data_dir();
    let target_dir = target.bundle().data_dir();
    let mut location_remap = HashMap::new();

    let source_base_url = source_dir.url().to_string();
    let target_base_url = target_dir.url().to_string();

    // List all files in source data_dir (excluding _bundlebase/ manifests)
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
            continue; // Skip files outside the data_dir
        };

        if relative.is_empty() {
            continue;
        }

        let source_file = source_dir.file(relative)?;
        let target_file = target_dir.writable_file(relative)?;

        // Read from source and write to target
        if let Some(stream) = source_file.read_stream().await? {
            let pinned: std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send>> =
                Box::pin(futures::StreamExt::map(stream, |r| {
                    r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                }));
            target_file.write_stream(pinned).await?;
        }

        // Build location remap: source URL → target URL
        let new_url = format!("{}/{}", target_base_url.trim_end_matches('/'), relative);
        if file_url != new_url {
            location_remap.insert(file_url, new_url);
        }
    }

    Ok(location_remap)
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_import_bundle_basic() {
        let cmd = parse_command("IMPORT BUNDLE './stations' AS stations ON lake_id = stations.lake_id")
            .expect("Failed to parse IMPORT BUNDLE");
        match cmd {
            BundleCommand::ImportBundle(ref c) => {
                assert_eq!(c.path, "./stations");
                assert_eq!(c.name, "stations");
                assert_eq!(c.expression, "lake_id = stations.lake_id");
                assert!(!c.flatten);
            }
            _ => panic!("Expected ImportBundle variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_import_bundle_flatten() {
        let cmd = parse_command(
            "IMPORT BUNDLE './stations' FLATTEN HISTORY AS stations ON lake_id = stations.lake_id",
        )
        .expect("Failed to parse IMPORT BUNDLE FLATTEN");
        match cmd {
            BundleCommand::ImportBundle(ref c) => {
                assert_eq!(c.path, "./stations");
                assert_eq!(c.name, "stations");
                assert!(c.flatten);
            }
            _ => panic!("Expected ImportBundle variant"),
        }
    }

    #[test]
    fn test_parse_import_bundle_case_insensitive() {
        let cmd = parse_command(
            "import bundle './data' as orders on id = orders.customer_id",
        )
        .expect("Failed to parse case-insensitive IMPORT BUNDLE");
        match cmd {
            BundleCommand::ImportBundle(ref c) => {
                assert_eq!(c.path, "./data");
                assert_eq!(c.name, "orders");
            }
            _ => panic!("Expected ImportBundle variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ImportBundleCommand::new(
            "./stations",
            "stations",
            "lake_id = stations.lake_id",
            false,
        );
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "IMPORT BUNDLE './stations' AS stations ON lake_id = stations.lake_id"
        );

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ImportBundle(ref c) => {
                assert_eq!(c.path, "./stations");
                assert_eq!(c.name, "stations");
                assert_eq!(c.expression, "lake_id = stations.lake_id");
                assert!(!c.flatten);
            }
            _ => panic!("Expected ImportBundle variant"),
        }
    }

    #[test]
    fn test_round_trip_flatten() {
        let cmd = ImportBundleCommand::new("./data", "orders", "id = orders.id", true);
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "IMPORT BUNDLE './data' FLATTEN HISTORY AS orders ON id = orders.id"
        );

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ImportBundle(ref c) => {
                assert!(c.flatten);
            }
            _ => panic!("Expected ImportBundle variant"),
        }
    }

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
        let pack_remap = HashMap::new();
        let loc_remap = HashMap::new();

        let op = AnyOperation::SetName(SetNameOp {
            name: "old name".to_string(),
        });
        assert!(remap_operation(&op, &pack_remap, &loc_remap).is_none());
    }

    #[test]
    fn test_remap_strips_set_description() {
        let pack_remap = HashMap::new();
        let loc_remap = HashMap::new();

        let op = AnyOperation::SetDescription(SetDescriptionOp {
            description: "old desc".to_string(),
        });
        assert!(remap_operation(&op, &pack_remap, &loc_remap).is_none());
    }

    #[test]
    fn test_remap_keeps_filter() {
        let pack_remap = HashMap::new();
        let loc_remap = HashMap::new();

        let op = AnyOperation::Filter(FilterOp::new(
            "SELECT * FROM bundle WHERE active = true",
            vec![],
        ));
        assert!(remap_operation(&op, &pack_remap, &loc_remap).is_some());
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
        let old_join_id: ObjectId = "00000000000000a5".try_into().unwrap();
        let new_join_id = ObjectId::generate();
        pack_remap.insert(old_join_id, new_join_id);

        let op = AnyOperation::CreateJoin(CreateJoinOp {
            id: old_join_id,
            name: "sub_join".to_string(),
            join_type: crate::bundle::pack::JoinTypeOption::Left,
            expression: "a = b".to_string(),
        });

        let remapped = remap_operation(&op, &pack_remap, &HashMap::new()).unwrap();
        match remapped {
            AnyOperation::CreateJoin(j) => assert_eq!(j.id, new_join_id),
            _ => panic!("Expected CreateJoin"),
        }
    }

    #[test]
    fn test_remap_drop_join_id() {
        let mut pack_remap = HashMap::new();
        let old_id: ObjectId = "00000000000000a5".try_into().unwrap();
        let new_id = ObjectId::generate();
        pack_remap.insert(old_id, new_id);

        let op = AnyOperation::DropJoin(DropJoinOp { id: old_id });
        let remapped = remap_operation(&op, &pack_remap, &HashMap::new()).unwrap();
        match remapped {
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

        let op = AnyOperation::RenameJoin(RenameJoinOp {
            id: old_id,
            new_name: "new_name".to_string(),
        });
        let remapped = remap_operation(&op, &pack_remap, &HashMap::new()).unwrap();
        match remapped {
            AnyOperation::RenameJoin(r) => {
                assert_eq!(r.id, new_id);
                assert_eq!(r.new_name, "new_name");
            }
            _ => panic!("Expected RenameJoin"),
        }
    }

    #[test]
    fn test_remap_replace_block_location() {
        let mut loc_remap = HashMap::new();
        loc_remap.insert("old://file.csv".to_string(), "new://file.csv".to_string());

        let op = AnyOperation::ReplaceBlock(ReplaceBlockOp {
            id: "00000000000000cc".try_into().unwrap(),
            new_location: "old://file.csv".to_string(),
            new_version: "2".to_string(),
            new_hash: "0".repeat(64),
            source_info: None,
        });

        let remapped = remap_operation(&op, &HashMap::new(), &loc_remap).unwrap();
        match remapped {
            AnyOperation::ReplaceBlock(r) => assert_eq!(r.new_location, "new://file.csv"),
            _ => panic!("Expected ReplaceBlock"),
        }
    }

    #[test]
    fn test_remap_index_blocks_path() {
        let mut loc_remap = HashMap::new();
        loc_remap.insert("old://index/col.idx".to_string(), "new://index/col.idx".to_string());

        let op = AnyOperation::IndexBlocks(IndexBlocksOp {
            index: ObjectId::generate(),
            blocks: vec![],
            path: "old://index/col.idx".to_string(),
            cardinality: 100,
            doc_count: None,
        });

        let remapped = remap_operation(&op, &HashMap::new(), &loc_remap).unwrap();
        match remapped {
            AnyOperation::IndexBlocks(i) => assert_eq!(i.path, "new://index/col.idx"),
            _ => panic!("Expected IndexBlocks"),
        }
    }

    #[test]
    fn test_remap_strips_save_config() {
        // SaveConfigOp requires a valid Scope which needs registry init.
        // Test via YAML deserialization instead.
        let yaml = "type: saveConfig\nscope: s3\nkey: region\nvalue: us-east-1";
        let op: AnyOperation = serde_yaml_ng::from_str(yaml).expect("deserialize");
        assert!(matches!(op, AnyOperation::SaveConfig(_)));
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_none());
    }

    #[test]
    fn test_remap_strips_create_view() {
        let op = AnyOperation::CreateView(CreateViewOp {
            id: ObjectId::generate(),
            name: "my_view".to_string(),
        });
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_none());
    }

    #[test]
    fn test_remap_keeps_rename_column() {
        let op = AnyOperation::RenameColumn(RenameColumnOp {
            id: "00000000000000c1".try_into().unwrap(),
            new_name: "new_col".to_string(),
        });
        let remapped = remap_operation(&op, &HashMap::new(), &HashMap::new());
        assert!(remapped.is_some());
        assert!(matches!(remapped.unwrap(), AnyOperation::RenameColumn(_)));
    }

    #[test]
    fn test_remap_keeps_drop_column() {
        let op = AnyOperation::DropColumn(DropColumnOp {
            id: "00000000000000c1".try_into().unwrap(),
        });
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_some());
    }

    #[test]
    fn test_remap_keeps_detach_block() {
        let op = AnyOperation::DetachBlock(DetachBlockOp {
            id: "00000000000000cc".try_into().unwrap(),
        });
        assert!(remap_operation(&op, &HashMap::new(), &HashMap::new()).is_some());
    }
}
