//! ImportTempFunction command implementation (runtime-only).
//!
//! Loads a function with runtime-only logic, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.
//!
//! Supports both single and wildcard/bulk discovery modes.

use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::connector_definition::{parse_from_url, to_from_url, Platform, Runner};
use crate::bundle::facade::BundleFacade;
use crate::bundle::function_definition::{parse_arrow_type_name, parse_function_name, FunctionEntry, FunctionKind};
use crate::function::lib_bridge::{load_ipc_manifest, load_lib_manifest, lookup_function_in_manifest};
use crate::NamespacedName;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Command to load a function with runtime-only logic (not persisted).
#[derive(Debug, Clone)]
pub struct ImportTempFunctionCommand {
    /// Full dotted function name, or `namespace.*` for wildcard mode
    pub name: String,
    /// Arrow type names for input parameters (None = auto-detect from manifest)
    pub input_types: Option<Vec<String>>,
    /// Arrow type name for return value (None = auto-detect from manifest)
    pub return_type: Option<String>,
    /// Runner type
    pub runner: Runner,
    /// Logic string
    pub logic: String,
    /// Platform
    pub platform: Platform,
    /// Scalar or aggregate
    pub kind: FunctionKind,
}

impl ImportTempFunctionCommand {
    pub fn new(
        name: impl Into<String>,
        input_types: Vec<String>,
        return_type: impl Into<String>,
        runner: Runner,
        logic: impl Into<String>,
        platform: Platform,
        kind: FunctionKind,
    ) -> Self {
        Self {
            name: name.into(),
            input_types: Some(input_types),
            return_type: Some(return_type.into()),
            runner,
            logic: logic.into(),
            platform,
            kind,
        }
    }

    pub fn new_auto_detect(
        name: impl Into<String>,
        runner: Runner,
        logic: impl Into<String>,
        platform: Platform,
        kind: FunctionKind,
    ) -> Self {
        Self {
            name: name.into(),
            input_types: None,
            return_type: None,
            runner,
            logic: logic.into(),
            platform,
            kind,
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
        let mut from_url = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::function_name => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    from_url = Some(extract_string_content(inner_pair.as_str())?);
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

        let from_url = from_url.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP FUNCTION missing FROM clause".into()
        })?;

        let (runner, logic) = parse_from_url(&from_url)?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        let kind: FunctionKind = match args.remove("type") {
            Some(s) => s.parse()?,
            None => FunctionKind::Scalar,
        };

        Ok(ImportTempFunctionCommand::new_auto_detect(name, runner, logic, platform, kind))
    }

    fn to_statement(&self) -> String {
        let from_url = to_from_url(self.runner, &self.logic);
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }
        if self.kind != FunctionKind::Scalar {
            with_parts.push(format!("type = {}", escape_string(&self.kind.to_string())));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT TEMP FUNCTION {} FROM {}",
                self.name,
                escape_string(&from_url)
            )
        } else {
            format!(
                "IMPORT TEMP FUNCTION {} FROM {} WITH ({})",
                self.name,
                escape_string(&from_url),
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
        if self.is_wildcard() {
            // Wildcard/bulk mode
            let namespace = self.wildcard_namespace().to_string();

            let manifest = match self.runner {
                Runner::Lib => load_lib_manifest(&self.logic)?,
                Runner::Ipc => load_ipc_manifest(&self.logic)?,
                other => {
                    return Err(format!(
                        "Wildcard function discovery only supports 'lib' and 'ipc' runners, got '{}'",
                        other
                    )
                    .into());
                }
            };

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
                let logic = format!("{}:{}", self.logic, symbol);
                let name = format!("{}.{}", namespace, manifest_entry.name);
                let namespaced: NamespacedName = name.parse()?;

                let entry = FunctionEntry {
                    id: crate::data::ObjectId::generate(),
                    name: namespaced,
                    input_types,
                    return_type,
                    runner: self.runner,
                    logic,
                    platform: self.platform.clone(),
                    temporary: true,
                    kind,
                };
                facade.import_temp_function(entry).await?;
                count += 1;
            }

            Ok(format!(
                "Loaded {} temporary function(s) from '{}'",
                count, self.logic
            ))
        } else {
            // Single function mode
            let namespaced: NamespacedName = self.name.parse()
                .map_err(|e: crate::BundlebaseError| e)?;

            let (input_type_names, return_type_name, kind) = match (&self.input_types, &self.return_type) {
                (Some(input_types), Some(return_type)) => {
                    (input_types.clone(), return_type.clone(), self.kind)
                }
                _ => {
                    let namespaced_name = parse_function_name(&self.name)?;
                    let func_name = &namespaced_name.name;
                    let entry = lookup_function_in_manifest(&self.runner, &self.logic, func_name)?;
                    let kind = match entry.kind.as_str() {
                        "aggregate" => FunctionKind::Aggregate,
                        _ => FunctionKind::Scalar,
                    };
                    (entry.input_types, entry.return_type, kind)
                }
            };

            let input_types = input_type_names.iter()
                .map(|s| parse_arrow_type_name(s))
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = parse_arrow_type_name(&return_type_name)?;
            let entry = FunctionEntry {
                id: crate::data::ObjectId::generate(),
                name: namespaced,
                input_types,
                return_type,
                runner: self.runner,
                logic: self.logic.clone(),
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
        let input = "IMPORT TEMP FUNCTION acme.double_val FROM 'python://mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
                assert!(c.return_type.is_none());
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_wildcard() {
        let input = "IMPORT TEMP FUNCTION acme.* FROM 'lib://./mylib.so'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "./mylib.so");
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_roundtrip() {
        let cmd = ImportTempFunctionCommand::new_auto_detect(
            "acme.double_val",
            Runner::Python,
            "mod:func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT TEMP FUNCTION acme.double_val FROM 'python://mod:func'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_roundtrip_aggregate() {
        let cmd = ImportTempFunctionCommand::new_auto_detect(
            "acme.my_sum",
            Runner::Python,
            "mod:MySum",
            Platform::any(),
            FunctionKind::Aggregate,
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("type = 'aggregate'"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.kind, FunctionKind::Aggregate);
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_default_platform() {
        let input = "IMPORT TEMP FUNCTION acme.double_val FROM 'python://mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }
}
