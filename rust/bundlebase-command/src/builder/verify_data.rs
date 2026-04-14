//! VerifyData command implementation.

use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::UpdateVersionOp;
use bundlebase::BundleBuilder;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use bundlebase_data::BlockId;
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::readable_file_from_path;
use bundlebase_io::IOReadFile;
use std::sync::Arc;

// Re-export verification types from core
pub use bundlebase::bundle::verification::{FileVerificationResult, VerificationResults};

// ============================================================================
// VerifyDataCommand
// ============================================================================

/// Command to verify the integrity of bundle data files.
#[derive(Debug, Clone)]
pub struct VerifyDataCommand {
    /// Whether to update versions for changed files
    pub update_versions: bool,
}

impl VerifyDataCommand {
    /// Create a new VerifyDataCommand.
    pub fn new(update_versions: bool) -> Self {
        Self { update_versions }
    }
}

impl CommandParsing for VerifyDataCommand {
    fn rule() -> Rule {
        Rule::verify_data_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // Check if UPDATE keyword is present
        let raw = pair.as_str().to_uppercase();
        let update_versions = raw.contains("UPDATE");

        Ok(VerifyDataCommand::new(update_versions))
    }

    fn to_statement(&self) -> String {
        if self.update_versions {
            "VERIFY DATA UPDATE".to_string()
        } else {
            "VERIFY DATA".to_string()
        }
    }
}

impl BundleBuilderCommand for VerifyDataCommand {
    type Output = VerificationResults;

    async fn execute(
        self: Box<Self>,
        builder: &BundleBuilder,
    ) -> Result<VerificationResults, BundlebaseError> {
        let mut results = Vec::new();
        let block_hashes = builder.bundle().build_block_hash_map();
        let block_locations = builder.bundle().build_block_location_map();
        let config = builder.bundle().config();

        // Collect block info first to avoid borrowing issues
        let blocks_to_verify: Vec<(BlockId, String, Option<String>, String)> = {
            let packs = builder.packs();
            let mut result = Vec::new();
            for pack in packs.values() {
                for block in pack.blocks() {
                    let block_id = *block.id();
                    let location = block_locations
                        .get(&block_id)
                        .cloned()
                        .unwrap_or_else(|| block.reader().url().to_string());
                    let expected_hash = block_hashes.get(&block_id).cloned();
                    let current_version = block.version();
                    result.push((block_id, location, expected_hash, current_version));
                }
            }
            result
        };

        // Process each block
        for (block_id, location, expected_hash, current_version) in blocks_to_verify {
            // Compute the actual hash
            let data_dir = builder.bundle().data_dir();
            let file = match readable_file_from_path(&location, data_dir, config.clone()).await {
                Ok(f) => f,
                Err(e) => {
                    results.push(FileVerificationResult {
                        location,
                        file_type: "data".to_string(),
                        expected_hash,
                        actual_hash: None,
                        passed: false,
                        error: Some(format!("Failed to open file: {}", e)),
                        version_updated: false,
                    });
                    continue;
                }
            };

            let actual_hash = match file.compute_hash().await {
                Ok(h) => h,
                Err(e) => {
                    results.push(FileVerificationResult {
                        location,
                        file_type: "data".to_string(),
                        expected_hash,
                        actual_hash: None,
                        passed: false,
                        error: Some(format!("Failed to compute hash: {}", e)),
                        version_updated: false,
                    });
                    continue;
                }
            };

            let hash_matches = expected_hash
                .as_ref()
                .map(|expected| expected == &actual_hash)
                .unwrap_or(true);

            if hash_matches {
                // Hash matches - check if version needs updating
                let mut version_updated = false;

                if self.update_versions {
                    // Read the current version from the file
                    let adapter_factory = Arc::clone(&builder.bundle().reader_factory);
                    let temp_id = BlockId::generate();
                    if let Ok(adapter) = adapter_factory.detect(&location, &temp_id, builder).await
                    {
                        if let Ok(file_version) = adapter.read_version().await {
                            if file_version != current_version {
                                // Version changed but hash matches - update version
                                let op = UpdateVersionOp::setup(block_id, file_version);
                                if builder
                                    .do_change(
                                        &format!("Update version for block {}", block_id),
                                        |b| {
                                            let op = op.clone();
                                            Box::pin(async move {
                                                b.apply_operation(op.into()).await?;
                                                Ok(())
                                            })
                                        },
                                    )
                                    .await
                                    .is_ok()
                                {
                                    version_updated = true;
                                }
                            }
                        }
                    }
                }

                results.push(FileVerificationResult {
                    location,
                    file_type: "data".to_string(),
                    expected_hash,
                    actual_hash: Some(actual_hash),
                    passed: true,
                    error: None,
                    version_updated,
                });
            } else {
                // Hash mismatch - verification failed
                results.push(FileVerificationResult {
                    location,
                    file_type: "data".to_string(),
                    expected_hash,
                    actual_hash: Some(actual_hash),
                    passed: false,
                    error: None,
                    version_updated: false,
                });
            }
        }

        // Verify index files exist
        let indexes = builder.indexes();
        for index_def in indexes.iter() {
            for indexed_blocks in index_def.all_indexed_blocks() {
                let path = indexed_blocks.path();
                let result = verify_index_exists(path, builder).await;
                results.push(result);
            }
        }

        let verification_results = VerificationResults::from_files(results);

        Ok(verification_results)
    }
}

/// Verify an index file exists.
async fn verify_index_exists(path: &str, builder: &BundleBuilder) -> FileVerificationResult {
    match ObjectStoreFile::from_str(path, builder.data_dir().as_ref(), builder.config()) {
        Ok(file) => match file.exists().await {
            Ok(true) => FileVerificationResult {
                location: path.to_string(),
                file_type: "index".to_string(),
                expected_hash: None,
                actual_hash: None,
                passed: true,
                error: None,
                version_updated: false,
            },
            Ok(false) => FileVerificationResult {
                location: path.to_string(),
                file_type: "index".to_string(),
                expected_hash: None,
                actual_hash: None,
                passed: false,
                error: Some("Index file not found".to_string()),
                version_updated: false,
            },
            Err(e) => FileVerificationResult {
                location: path.to_string(),
                file_type: "index".to_string(),
                expected_hash: None,
                actual_hash: None,
                passed: false,
                error: Some(format!("Failed to check index file: {}", e)),
                version_updated: false,
            },
        },
        Err(e) => FileVerificationResult {
            location: path.to_string(),
            file_type: "index".to_string(),
            expected_hash: None,
            actual_hash: None,
            passed: false,
            error: Some(format!("Failed to create file handle: {}", e)),
            version_updated: false,
        },
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_verify_data() {
        let input = "VERIFY DATA";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::VerifyData(c) => {
                assert!(!c.update_versions);
            }
            _ => panic!("Expected VerifyData variant"),
        }
    }

    #[test]
    fn test_parse_verify_data_update() {
        let input = "VERIFY DATA UPDATE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::VerifyData(c) => {
                assert!(c.update_versions);
            }
            _ => panic!("Expected VerifyData variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = VerifyDataCommand::new(false);
        let statement = cmd.to_statement();
        assert_eq!(statement, "VERIFY DATA");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::VerifyData(c) => {
                assert!(!c.update_versions);
            }
            _ => panic!("Expected VerifyData variant"),
        }
    }

    #[test]
    fn test_round_trip_update() {
        let cmd = VerifyDataCommand::new(true);
        let statement = cmd.to_statement();
        assert_eq!(statement, "VERIFY DATA UPDATE");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::VerifyData(c) => {
                assert!(c.update_versions);
            }
            _ => panic!("Expected VerifyData variant"),
        }
    }
}
