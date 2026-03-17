//! ImportTempFunction command implementation (runtime-only).
//!
//! Loads a function with runtime-only logic, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.
//!
//! Supports both single and wildcard/bulk discovery modes.

use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::connector_definition::Platform;
use crate::bundle::logic_runtime::LogicRuntime;
use crate::bundle::facade::BundleFacade;
use crate::bundle::function_definition::{parse_arrow_type_name, FunctionEntry, FunctionKind};
use crate::NamespacedName;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Command to load a function with runtime-only logic (not persisted).
///
/// Types and kind are always auto-detected from the function's manifest.
#[derive(Debug, Clone)]
pub struct ImportTempFunctionCommand {
    /// Full dotted function name, or `namespace.*` for wildcard mode
    pub name: String,
    /// From string (e.g. "python::mod:func")
    pub from: String,
    /// Platform
    pub platform: Platform,
}

impl ImportTempFunctionCommand {
    pub fn new(
        name: impl Into<String>,
        from: impl Into<String>,
        platform: Platform,
    ) -> Self {
        Self {
            name: name.into(),
            from: from.into(),
            platform,
        }
    }

    /// Returns true if this is a wildcard/bulk discovery command.
    pub fn is_wildcard(&self) -> bool {
        self.name.ends_with(".*")
    }

    /// Returns the namespace for wildcard mode.
    fn wildcard_namespace(&self) -> &str {
        &self.name[..self.name.len() - 2]
    }

    /// Returns the Arrow schema for this command's output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    /// Returns the expected output shape.
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for ImportTempFunctionCommand {
    fn rule() -> Rule {
        Rule::import_temp_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut from = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::function_name => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    from = Some(extract_string_content(inner_pair.as_str())?);
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(part.as_str().to_string());
                                    }
                                    Rule::quoted_string => {
                                        value = Some(extract_string_content(part.as_str())?);
                                    }
                                    _ => {}
                                }
                            }
                            if let (Some(k), Some(v)) = (key, value) {
                                args.insert(k, v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP FUNCTION missing function name".into()
        })?;

        let from = from.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP FUNCTION missing FROM clause".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(ImportTempFunctionCommand::new(name, from, platform))
    }

    fn to_statement(&self) -> String {
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT TEMP FUNCTION {} FROM {}",
                self.name,
                escape_string(&self.from)
            )
        } else {
            format!(
                "IMPORT TEMP FUNCTION {} FROM {} WITH ({})",
                self.name,
                escape_string(&self.from),
                with_parts.join(", ")
            )
        }
    }
}

#[async_trait]
impl BundleFacadeCommand for ImportTempFunctionCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let from = LogicRuntime::parse_from(&self.from)?;
        from.validate_logic()?;

        if self.is_wildcard() {
            // Wildcard/bulk mode
            let namespace = self.wildcard_namespace().to_string();

            let manifest = from.load_manifest()?
                .ok_or_else(|| -> BundlebaseError {
                    format!(
                        "Wildcard function discovery not supported for '{}' runner",
                        from.runtime_name()
                    )
                    .into()
                })?;

            let mut count = 0;
            for manifest_entry in &manifest.functions {
                let input_types = manifest_entry
                    .input_types
                    .iter()
                    .map(|s| parse_arrow_type_name(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_type = parse_arrow_type_name(&manifest_entry.return_type)?;
                let kind: FunctionKind = manifest_entry.kind.parse()?;

                let symbol = manifest_entry.symbol.as_deref().unwrap_or(&manifest_entry.name);
                let func_logic = format!("{}:{}", from.to_logic_string(), symbol);
                let func_from = LogicRuntime::parse_from(&format!("{}::{}", from.runtime_name(), func_logic))?;
                let name = format!("{}.{}", namespace, manifest_entry.name);
                let namespaced: NamespacedName = name.parse()?;

                let entry = FunctionEntry {
                    id: crate::data::ObjectId::generate(),
                    name: namespaced,
                    input_types,
                    return_type,
                    from: func_from,
                    platform: self.platform.clone(),
                    temporary: true,
                    kind,
                };
                facade.import_temp_function(entry).await?;
                count += 1;
            }

            Ok(format!(
                "Loaded {} temporary function(s) from '{}'",
                count, from.to_logic_string()
            ))
        } else {
            // Single function mode — auto-detect types from manifest
            let namespaced: NamespacedName = self.name.parse()
                .map_err(|e: crate::BundlebaseError| e)?;

            // Use the symbol from the from string (e.g., "MySum" from "module:MySum")
            // rather than the SQL name, since the SQL name may differ from the manifest entry.
            let logic_str = from.to_logic_string();
            let symbol = logic_str.rsplit(':').next().unwrap_or(&logic_str);
            let manifest_entry = from.lookup_function_in_manifest(symbol)?;
            let kind: FunctionKind = match manifest_entry.kind.as_str() {
                "aggregate" => FunctionKind::Aggregate,
                _ => FunctionKind::Scalar,
            };

            let input_types = manifest_entry.input_types.iter()
                .map(|s| parse_arrow_type_name(s))
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = parse_arrow_type_name(&manifest_entry.return_type)?;
            let entry = FunctionEntry {
                id: crate::data::ObjectId::generate(),
                name: namespaced,
                input_types,
                return_type,
                from: from.clone(),
                platform: self.platform.clone(),
                temporary: true,
                kind,
            };
            facade.import_temp_function(entry).await?;
            Ok(format!("Loaded temporary function: {}", self.name))
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_import_temp_function() {
        let input = "IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "python::mod:func");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_wildcard() {
        let input = "IMPORT TEMP FUNCTION acme.* FROM 'ffi::./mylib.so'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.from, "ffi::./mylib.so");
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_roundtrip() {
        let cmd = ImportTempFunctionCommand::new(
            "acme.double_val",
            "python::mod:func",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "python::mod:func");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_default_platform() {
        let input = "IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }
}
