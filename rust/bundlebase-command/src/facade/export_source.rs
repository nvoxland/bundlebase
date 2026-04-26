//! EXPORT SOURCE command — copy a connector's bundled source archive to disk.
//!
//! Recipients of a bundle that was built with `IMPORT CONNECTOR ... WITH (src = '...')`
//! can use this to extract the original source for audit, rebuild, or fork.

use crate::parser::{escape_string, extract_string_content};
use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use std::sync::Arc;

/// Copy `connector.src` (a bundle-relative path under `data_dir`) to `path`
/// on the local filesystem.
#[derive(Debug, Clone)]
pub struct ExportSourceCommand {
    pub connector_name: String,
    pub path: String,
}

impl ExportSourceCommand {
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for ExportSourceCommand {
    fn rule() -> Rule {
        Rule::export_source_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut connector_name = None;
        let mut path = None;
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::dotted_identifier => {
                    connector_name = Some(inner.as_str().to_string());
                }
                Rule::quoted_string => {
                    path = Some(extract_string_content(inner.as_str())?);
                }
                _ => {}
            }
        }
        let connector_name = connector_name.ok_or_else(|| -> BundlebaseError {
            "EXPORT SOURCE: missing connector name".into()
        })?;
        let path = path
            .ok_or_else(|| -> BundlebaseError { "EXPORT SOURCE: missing TO path".into() })?;
        Ok(ExportSourceCommand {
            connector_name,
            path,
        })
    }

    fn to_statement(&self) -> String {
        format!(
            "EXPORT SOURCE {} TO {}",
            self.connector_name,
            escape_string(&self.path)
        )
    }
}

impl BundleFacadeCommand for ExportSourceCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        // Find any registered connector entry with this name. Multi-platform
        // imports share the same `src` across all entries, so the first
        // matching entry's src is fine — we don't need to resolve to host.
        let entries: Vec<_> = facade
            .connector_registry()
            .read()
            .entries()
            .iter()
            .filter(|e| e.name.to_string() == self.connector_name)
            .cloned()
            .collect();

        if entries.is_empty() {
            return Err(format!("No connector named '{}' is registered.", self.connector_name).into());
        }

        // Find the first src — they should all be the same when produced by
        // one IMPORT CONNECTOR call, but tolerate variation by picking any.
        let src = entries
            .iter()
            .find_map(|e| e.src.clone())
            .ok_or_else(|| -> BundlebaseError {
                format!(
                    "Connector '{}' has no bundled source archive (was imported without WITH (src = ...)).",
                    self.connector_name
                )
                .into()
            })?;

        let data_dir = facade.data_dir();
        let src_file = data_dir.file(&src)?;
        let bytes = src_file.read_bytes().await?.ok_or_else(|| -> BundlebaseError {
            format!(
                "Connector '{}' references src '{}' but the file is missing from data_dir.",
                self.connector_name, src
            )
            .into()
        })?;

        // Resolve the output path: absolute paths go straight to disk; relative
        // paths resolve against cwd.
        let abs_path = if std::path::Path::new(&self.path).is_absolute() {
            std::path::PathBuf::from(&self.path)
        } else {
            std::env::current_dir()
                .map_err(|e| BundlebaseError::from(format!("Failed to get cwd: {}", e)))?
                .join(&self.path)
        };
        if let Some(parent) = abs_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to create parent directory '{}': {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        let n = bytes.len();
        tokio::fs::write(&abs_path, &bytes).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to write source archive to '{}': {}",
                abs_path.display(),
                e
            ))
        })?;

        Ok(format!(
            "Exported {} bytes of '{}' source to '{}'",
            n,
            self.connector_name,
            abs_path.display()
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_basic() {
        let cmd = parse_command("EXPORT SOURCE acme.weather TO '/tmp/out.zip'").unwrap();
        match cmd {
            BundleCommand::ExportSource(c) => {
                assert_eq!(c.connector_name, "acme.weather");
                assert_eq!(c.path, "/tmp/out.zip");
            }
            _ => panic!("expected ExportSource"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ExportSourceCommand {
            connector_name: "acme.weather".to_string(),
            path: "out.zip".to_string(),
        };
        let stmt = cmd.to_statement();
        assert_eq!(stmt, "EXPORT SOURCE acme.weather TO 'out.zip'");
        let parsed = parse_command(&stmt).unwrap();
        match parsed {
            BundleCommand::ExportSource(c) => {
                assert_eq!(c.connector_name, "acme.weather");
                assert_eq!(c.path, "out.zip");
            }
            _ => panic!("expected ExportSource"),
        }
    }

    #[test]
    fn test_case_insensitive() {
        let cmd = parse_command("export source acme.weather to '/tmp/out.zip'").unwrap();
        match cmd {
            BundleCommand::ExportSource(_) => {}
            _ => panic!("expected ExportSource"),
        }
    }
}
