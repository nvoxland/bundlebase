//! VerifyData command implementation.

use crate::bundle::command::{CommandParsing, Rule, CommandResponse};
use crate::impl_dyn_command_response;
use crate::bundle::facade::BundleFacade;
use crate::bundle::operation::UpdateVersionOp;
use crate::io::readable_file_from_path;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use crate::data::ObjectId;
use crate::BundlebaseError;
use arrow::array::{ArrayRef, BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::sync::Arc;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

// ============================================================================
// Verification Result Types
// ============================================================================

/// Result of verifying a single file
#[derive(Debug, Clone)]
pub struct FileVerificationResult {
    pub location: String,
    pub file_type: String, // "data" or "index"
    pub expected_hash: Option<String>,
    pub actual_hash: Option<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub version_updated: bool, // True if version was updated
}

/// Complete verification results for a bundle
#[derive(Debug, Clone)]
pub struct VerificationResults {
    pub files: Vec<FileVerificationResult>,
    pub passed_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    pub versions_updated_count: usize,
    pub all_passed: bool,
}

impl CommandResponse for VerificationResults {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("location", DataType::Utf8, false),
            Field::new("file_type", DataType::Utf8, false),
            Field::new("expected_hash", DataType::Utf8, true),
            Field::new("actual_hash", DataType::Utf8, true),
            Field::new("passed", DataType::Boolean, false),
            Field::new("error", DataType::Utf8, true),
            Field::new("version_updated", DataType::Boolean, false),
        ]))
    }

    fn output_shape() -> crate::bundle::command::response::OutputShape {
        crate::bundle::command::response::OutputShape::Table
    }

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let files = &self.files;

        let location: ArrayRef = Arc::new(StringArray::from(
            files.iter().map(|r| r.location.as_str()).collect::<Vec<_>>(),
        ));
        let file_type: ArrayRef = Arc::new(StringArray::from(
            files.iter().map(|r| r.file_type.as_str()).collect::<Vec<_>>(),
        ));
        let expected_hash: ArrayRef = Arc::new(StringArray::from(
            files.iter()
                .map(|r| r.expected_hash.as_deref())
                .collect::<Vec<_>>(),
        ));
        let actual_hash: ArrayRef = Arc::new(StringArray::from(
            files.iter()
                .map(|r| r.actual_hash.as_deref())
                .collect::<Vec<_>>(),
        ));
        let passed: ArrayRef = Arc::new(BooleanArray::from(
            files.iter().map(|r| r.passed).collect::<Vec<_>>(),
        ));
        let error: ArrayRef = Arc::new(StringArray::from(
            files.iter().map(|r| r.error.as_deref()).collect::<Vec<_>>(),
        ));
        let version_updated: ArrayRef = Arc::new(BooleanArray::from(
            files.iter().map(|r| r.version_updated).collect::<Vec<_>>(),
        ));

        RecordBatch::try_new(
            Self::schema(),
            vec![
                location,
                file_type,
                expected_hash,
                actual_hash,
                passed,
                error,
                version_updated,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }

    impl_dyn_command_response!(VerificationResults);
}

impl std::fmt::Display for VerificationResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.all_passed {
            write!(f, "All {} files verified successfully", self.passed_count)
        } else {
            writeln!(
                f,
                "Verification: {} passed, {} failed",
                self.passed_count, self.failed_count
            )?;
            for file in self.files.iter().filter(|file| !file.passed) {
                write!(f, "  FAILED: {}", file.location)?;
                if let Some(ref err) = file.error {
                    write!(f, " ({})", err)?;
                }
                writeln!(f)?;
            }
            Ok(())
        }
    }
}

impl VerificationResults {
    /// Create a new VerificationResults from a list of file results
    pub fn from_files(files: Vec<FileVerificationResult>) -> Self {
        let passed_count = files.iter().filter(|f| f.passed).count();
        let failed_count = files.iter().filter(|f| !f.passed).count();
        let skipped_count = files
            .iter()
            .filter(|f| f.passed && f.expected_hash.is_none())
            .count();
        let versions_updated_count = files.iter().filter(|f| f.version_updated).count();
        let all_passed = failed_count == 0;

        Self {
            files,
            passed_count,
            failed_count,
            skipped_count,
            versions_updated_count,
            all_passed,
        }
    }

    /// Check verification results and return error if any files failed.
    ///
    /// Throws an error if:
    /// - Any file has a checksum mismatch (hash doesn't match)
    /// - Any file has a verification error
    pub fn check(&self) -> Result<(), BundlebaseError> {
        let failures: Vec<&FileVerificationResult> =
            self.files.iter().filter(|f| !f.passed).collect();

        if failures.is_empty() {
            return Ok(());
        }

        let messages: Vec<String> = failures
            .iter()
            .map(|f| {
                if let Some(ref err) = f.error {
                    format!("{}: {}", f.location, err)
                } else if f.expected_hash != f.actual_hash {
                    format!(
                        "{}: hash mismatch (expected {}, got {})",
                        f.location,
                        f.expected_hash.as_deref().unwrap_or("none"),
                        f.actual_hash.as_deref().unwrap_or("none")
                    )
                } else {
                    format!("{}: verification failed", f.location)
                }
            })
            .collect();

        Err(BundlebaseError::from(format!(
            "Data verification failed for {} file(s):\n{}",
            failures.len(),
            messages.join("\n")
        )))
    }
}

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

#[async_trait]
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
            // Skip function:// URLs (generated data has no file to verify)
            if location.starts_with("function://") {
                results.push(FileVerificationResult {
                    location,
                    file_type: "data".to_string(),
                    expected_hash: None,
                    actual_hash: None,
                    passed: true,
                    error: None,
                    version_updated: false,
                });
                continue;
            }

            // Compute the actual hash
            let data_dir = builder.bundle().data_dir();
            let file = match readable_file_from_path(&location, data_dir, config.clone()) {
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
                    let temp_id = ObjectId::generate();
                    if let Ok(adapter) = adapter_factory
                        .reader(&location, &temp_id, builder, None, None, None)
                        .await
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
        let indexes = builder.bundle().indexes().read().clone();
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
