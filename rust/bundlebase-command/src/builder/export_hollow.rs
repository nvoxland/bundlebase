//! Export Hollow command implementation.
//!
//! ExportHollowCommand creates a "hollow" bundle at the target path — containing
//! the source definitions, always-update/always-delete rules, column operations,
//! and structure, but no attached data. The hollow bundle can be shared so
//! recipients can re-fetch the raw data themselves.

use crate::parser::extract_string_content;
use crate::{BundleBuilderCommand, CommandParsing, Rule};
use bundlebase::{AnyOperation, HollowContext, BundleBuilder, BundlebaseError};
use bundlebase::bundle::BundleFacade;
use bundlebase::bundle::operation::Operation;
use bundlebase_data::ObjectId;
use bundlebase_common::ColumnId;
use arrow::datatypes::DataType;
use std::collections::HashMap;

/// Command to export a hollow bundle.
///
/// Walks all operations in the bundle's history, strips data-containing operations
/// (AttachBlock, DetachBlock, etc.), fills expected schemas from AttachBlock history,
/// creates a new bundle at the target path, applies the remaining ops, and commits.
#[derive(Debug, Clone)]
pub struct ExportHollowCommand {
    pub path: String,
}

impl CommandParsing for ExportHollowCommand {
    fn rule() -> Rule {
        Rule::export_hollow_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                path = Some(extract_string_content(inner.as_str())?);
            }
        }
        let path = path.ok_or_else(|| BundlebaseError::from("EXPORT HOLLOW TO: missing target path"))?;
        Ok(ExportHollowCommand { path })
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        format!("EXPORT HOLLOW TO {}", escape_string(&self.path))
    }
}

impl BundleBuilderCommand for ExportHollowCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let all_ops = builder.operations();

        // Pass 1: Build HollowContext by scanning AttachBlock ops.
        // For each source, collect the most recent schema seen (column name, id, type).
        let mut source_schemas: HashMap<ObjectId, Vec<(String, ColumnId, DataType)>> = HashMap::new();

        for op in &all_ops {
            if let AnyOperation::AttachBlock(attach) = op {
                if let Some(ref source_info) = attach.source_info {
                    if let Some(ref schema) = attach.schema {
                        let cols: Vec<(String, ColumnId, DataType)> = schema
                            .fields()
                            .iter()
                            .zip(attach.column_ids.iter())
                            .map(|(f, id)| (f.name().clone(), *id, f.data_type().clone()))
                            .collect();
                        // Most recent AttachBlock wins (overwrite any previous entry)
                        source_schemas.insert(source_info.id, cols);
                    }
                }
            }
        }

        let context = HollowContext { source_schemas };

        // Pass 2: Apply to_hollow() to each op, collecting the ones to keep.
        let hollow_ops: Vec<AnyOperation> = all_ops
            .iter()
            .filter_map(|op| op.to_hollow(&context))
            .collect();

        // Create target bundle and apply hollow ops.
        let hollow_builder = BundleBuilder::create(&self.path, None).await?;

        for op in hollow_ops {
            hollow_builder.apply_operation(op).await?;
        }

        hollow_builder.commit("Hollow export").await?;

        Ok(format!("Hollow bundle created at '{}'", self.path))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_export_hollow_basic() {
        let cmd = parse_command("EXPORT HOLLOW TO 'path/hollow'")
            .expect("Failed to parse EXPORT HOLLOW");
        match cmd {
            BundleCommand::ExportHollow(ref c) => {
                assert_eq!(c.path, "path/hollow");
            }
            _ => panic!("Expected ExportHollow variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_export_hollow_tar() {
        let cmd = parse_command("EXPORT HOLLOW TO 'path/hollow.tar'")
            .expect("Failed to parse EXPORT HOLLOW with tar path");
        match cmd {
            BundleCommand::ExportHollow(ref c) => {
                assert_eq!(c.path, "path/hollow.tar");
            }
            _ => panic!("Expected ExportHollow variant"),
        }
    }

    #[test]
    fn test_parse_export_hollow_case_insensitive() {
        let cmd = parse_command("export hollow to 'bundle.tar'")
            .expect("Failed to parse case-insensitive EXPORT HOLLOW");
        match cmd {
            BundleCommand::ExportHollow(ref c) => {
                assert_eq!(c.path, "bundle.tar");
            }
            _ => panic!("Expected ExportHollow variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ExportHollowCommand {
            path: "path/hollow".to_string(),
        };
        let statement = cmd.to_statement();
        assert_eq!(statement, "EXPORT HOLLOW TO 'path/hollow'");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ExportHollow(ref c) => {
                assert_eq!(c.path, "path/hollow");
            }
            _ => panic!("Expected ExportHollow variant"),
        }
    }
}
