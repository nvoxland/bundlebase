//! Export Empty command implementation.
//!
//! ExportEmptyCommand creates an "empty" bundle at the target path — containing
//! the source definitions, always-update/always-delete rules, column operations,
//! and structure, but no attached data. The empty bundle can be shared so
//! recipients can re-fetch the raw data themselves.

use crate::parser::extract_string_content;
use crate::{BundleBuilderCommand, CommandParsing, Rule};
use arrow::datatypes::DataType;
use bundlebase::bundle::operation::Operation;
use bundlebase::bundle::BundleFacade;
use bundlebase::{AnyOperation, BundleBuilder, BundlebaseError, EmptyContext};
use bundlebase_common::ColumnId;
use bundlebase_data::ObjectId;
use std::collections::HashMap;

/// Command to export an empty bundle.
///
/// Walks all operations in the bundle's history, strips data-containing operations
/// (AttachBlock, DetachBlock, etc.), fills expected schemas from AttachBlock history,
/// creates a new bundle at the target path, applies the remaining ops, and commits.
#[derive(Debug, Clone)]
pub struct ExportEmptyCommand {
    pub path: String,
}

impl CommandParsing for ExportEmptyCommand {
    fn rule() -> Rule {
        Rule::export_empty_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                path = Some(extract_string_content(inner.as_str())?);
            }
        }
        let path =
            path.ok_or_else(|| BundlebaseError::from("EXPORT EMPTY TO: missing target path"))?;
        Ok(ExportEmptyCommand { path })
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        format!("EXPORT EMPTY TO {}", escape_string(&self.path))
    }
}

impl BundleBuilderCommand for ExportEmptyCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Reject relative paths: they would resolve against the CLI's working
        // directory, which is rarely what the SQL author intended (the
        // bundle's own directory is more natural but inconsistent with how
        // the CLI itself resolves paths). Force callers to be explicit.
        if !self.path.contains(':') && !std::path::Path::new(&self.path).is_absolute() {
            return Err(BundlebaseError::from(format!(
                "EXPORT EMPTY path '{}' must be absolute or a full URL (e.g. file:///…, s3://…)",
                self.path
            )));
        }

        let all_ops = builder.operations();

        // Pass 1: Build EmptyContext by scanning AttachBlock ops.
        // For each source, collect the most recent schema seen (column name, id, type).
        let mut source_schemas: HashMap<ObjectId, Vec<(String, ColumnId, DataType)>> =
            HashMap::new();

        for op in &all_ops {
            if let AnyOperation::AttachBlock(attach) = op {
                if let Some(ref source_info) = attach.source_info {
                    if let Some(ref schema) = attach.schema_cache {
                        let cols: Vec<(String, ColumnId, DataType)> = schema
                            .fields()
                            .iter()
                            .zip(attach.column_ids_cache.iter())
                            .map(|(f, id)| (f.name().clone(), *id, f.data_type().clone()))
                            .collect();
                        // Most recent AttachBlock wins (overwrite any previous entry)
                        source_schemas.insert(source_info.id, cols);
                    }
                }
            }
        }

        let context = EmptyContext { source_schemas };

        // Pass 2: Apply to_empty() to each op, collecting the ones to keep.
        let empty_ops: Vec<AnyOperation> = all_ops
            .iter()
            .filter_map(|op| op.to_empty(&context))
            .collect();

        // Create target bundle and apply empty ops within a change context.
        let empty_builder = BundleBuilder::create(&self.path, None).await?;

        // Copy any bundled connector / function binaries from the source
        // bundle's data directory into the empty bundle's data directory.
        // ImportConnectorOp / ImportFunctionOp reference these by relative
        // path; without the actual bytes the empty bundle would fail to load
        // the connector at fetch time.
        copy_bundled_runtime_files(&empty_ops, builder, &empty_builder).await?;

        empty_builder
            .do_change("Empty export", |b| {
                Box::pin(async move {
                    for op in empty_ops {
                        b.apply_operation(op).await?;
                    }
                    Ok(())
                })
            })
            .await?;

        empty_builder.commit("Empty export").await?;

        Ok(format!("Empty bundle created at '{}'", self.path))
    }
}

/// Copy bundle-relative files referenced by ImportConnector / ImportFunction
/// ops from the source bundle's data directory into the empty bundle's. Only
/// relative paths (the content-addressed `xx/<hash>.udf.bin` form produced by
/// `copy_into_bundle`) are copied — absolute paths and non-file runtimes are
/// left untouched.
async fn copy_bundled_runtime_files(
    ops: &[AnyOperation],
    src_builder: &BundleBuilder,
    dst_builder: &BundleBuilder,
) -> Result<(), BundlebaseError> {
    let src_dir = src_builder.bundle().data_dir();
    let dst_dir = dst_builder.bundle().data_dir();

    // Collect every bundle-relative path we need to copy across all ops, then
    // de-dup so a fat connector with one shared `src` zip only copies it once.
    let mut paths: Vec<String> = Vec::new();
    for op in ops {
        match op {
            AnyOperation::ImportConnector(o) => {
                if let Some(p) = o.from.file_path() {
                    paths.push(p.to_string());
                }
                if let Some(s) = &o.src {
                    paths.push(s.clone());
                }
            }
            AnyOperation::ImportFunction(o) => {
                if let Some(p) = o.from.file_path() {
                    paths.push(p.to_string());
                }
            }
            _ => {}
        }
    }
    paths.sort();
    paths.dedup();

    for file_path in paths {
        // Skip absolute / parent-relative paths — those reference files
        // outside the bundle and aren't ours to copy.
        if file_path.starts_with('/')
            || file_path.starts_with("./")
            || file_path.starts_with("../")
        {
            continue;
        }

        let src_file = src_dir.file(&file_path)?;
        let bytes = src_file
            .read_bytes()
            .await?
            .ok_or_else(|| BundlebaseError::from(format!(
                "Bundled runtime file '{}' is missing from source bundle; cannot include it in empty export",
                file_path
            )))?;
        let dst_file = dst_dir.writable_file(&file_path)?;
        dst_file.write(bytes).await?;
    }
    Ok(())
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_export_empty_basic() {
        let cmd =
            parse_command("EXPORT EMPTY TO 'path/empty'").expect("Failed to parse EXPORT EMPTY");
        match cmd {
            BundleCommand::ExportEmpty(ref c) => {
                assert_eq!(c.path, "path/empty");
            }
            _ => panic!("Expected ExportEmpty variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_export_empty_tar() {
        let cmd = parse_command("EXPORT EMPTY TO 'path/empty.tar'")
            .expect("Failed to parse EXPORT EMPTY with tar path");
        match cmd {
            BundleCommand::ExportEmpty(ref c) => {
                assert_eq!(c.path, "path/empty.tar");
            }
            _ => panic!("Expected ExportEmpty variant"),
        }
    }

    #[test]
    fn test_parse_export_empty_case_insensitive() {
        let cmd = parse_command("export empty to 'bundle.tar'")
            .expect("Failed to parse case-insensitive EXPORT EMPTY");
        match cmd {
            BundleCommand::ExportEmpty(ref c) => {
                assert_eq!(c.path, "bundle.tar");
            }
            _ => panic!("Expected ExportEmpty variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ExportEmptyCommand {
            path: "path/empty".to_string(),
        };
        let statement = cmd.to_statement();
        assert_eq!(statement, "EXPORT EMPTY TO 'path/empty'");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ExportEmpty(ref c) => {
                assert_eq!(c.path, "path/empty");
            }
            _ => panic!("Expected ExportEmpty variant"),
        }
    }
}
