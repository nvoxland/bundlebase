//! VerifyData command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::{FileVerificationResult, VerificationResults};
use crate::bundle::operation::UpdateVersionOp;
use crate::io::readable_file_from_path;
use crate::data::ObjectId;
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;
use super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

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

#[async_trait]
impl BundleBuilderCommand for VerifyDataCommand {
    type Output = VerificationResults;

    async fn execute(
        self: Box<Self>,
        builder: &mut BundleBuilder,
    ) -> Result<VerificationResults, BundlebaseError> {
        let mut results = Vec::new();
        let block_hashes = builder.bundle().build_block_hash_map();
        let block_locations = builder.bundle().build_block_location_map();

        // Collect block info first to avoid borrowing issues
        let blocks_to_verify: Vec<(ObjectId, String, Option<String>, String)> = {
            let packs = builder.bundle().packs().read().clone();
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
            // Compute actual hash
            let data_dir = builder.bundle().data_dir();
            let config = builder.bundle().config();
            let hash_result = compute_file_hash(&location, data_dir, config).await;

            match hash_result {
                Ok(actual_hash) => {
                    let passed = expected_hash
                        .as_ref()
                        .map(|expected| expected == &actual_hash)
                        .unwrap_or(true);

                    // Check if version needs updating (hash changed from stored version)
                    let version_updated = if self.update_versions && passed {
                        // If there's an expected hash and actual matches, check if version includes it
                        let needs_update = expected_hash.is_some()
                            && expected_hash.as_ref() != Some(&actual_hash)
                            || !current_version.contains(&actual_hash[..8]);

                        if needs_update {
                            // Create update operation
                            let op = UpdateVersionOp::setup(block_id, actual_hash.clone());
                            builder.apply_operation(op.into()).await?;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    results.push(FileVerificationResult {
                        location,
                        file_type: "data".to_string(),
                        expected_hash,
                        actual_hash: Some(actual_hash),
                        passed,
                        error: None,
                        version_updated,
                    });
                }
                Err(e) => {
                    results.push(FileVerificationResult {
                        location,
                        file_type: "data".to_string(),
                        expected_hash,
                        actual_hash: None,
                        passed: false,
                        error: Some(e.to_string()),
                        version_updated: false,
                    });
                }
            }
        }

        let verification_results = VerificationResults::from_files(results);

        if verification_results.all_passed {
            info!("All {} files verified successfully", verification_results.passed_count);
        } else {
            info!(
                "Verification complete: {} passed, {} failed",
                verification_results.passed_count, verification_results.failed_count
            );
        }

        Ok(verification_results)
    }
}

/// Compute the SHA256 hash of a file
async fn compute_file_hash(
    location: &str,
    data_dir: &dyn crate::io::IOReadDir,
    config: std::sync::Arc<crate::BundleConfig>,
) -> Result<String, BundlebaseError> {
    let file = readable_file_from_path(location, data_dir, config)?;
    file.compute_hash().await
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

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
