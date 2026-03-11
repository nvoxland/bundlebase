//! ImportFunction command implementation (persistent).
//!
//! Handles both single function loading and wildcard/bulk discovery
//! (e.g., `IMPORT FUNCTION acme.* FROM 'lib://./mylib.so'`).

use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::connector_definition::{parse_from_url, to_from_url, Platform, Runner};
use crate::bundle::function_definition::{parse_arrow_type_name, parse_function_name, FunctionKind};
use crate::bundle::operation::ImportFunctionOp;
use crate::function::lib_bridge::{load_ipc_manifest, load_lib_manifest, lookup_function_in_manifest};
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to define a named function with its logic.
///
/// Supports two modes:
/// - **Single**: `IMPORT FUNCTION acme.double_val FROM 'ipc://./my_func'`
/// - **Wildcard**: `IMPORT FUNCTION acme.* FROM 'lib://./mylib.so'` (bulk discovery)
#[derive(Debug, Clone)]
pub struct ImportFunctionCommand {
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

impl ImportFunctionCommand {
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

    /// Returns true if this is a wildcard/bulk discovery command (name ends with `.*`).
    pub fn is_wildcard(&self) -> bool {
        self.name.ends_with(".*")
    }

    /// Returns the namespace for wildcard mode (strips `.*` suffix).
    fn wildcard_namespace(&self) -> &str {
        &self.name[..self.name.len() - 2]
    }
}

impl CommandParsing for ImportFunctionCommand {
    fn rule() -> Rule {
        Rule::import_function_stmt
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
            "IMPORT FUNCTION missing function name".into()
        })?;

        let from_url = from_url.ok_or_else(|| -> BundlebaseError {
            "IMPORT FUNCTION missing FROM clause".into()
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

        Ok(ImportFunctionCommand::new_auto_detect(name, runner, logic, platform, kind))
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
                "IMPORT FUNCTION {} FROM {}",
                self.name,
                escape_string(&from_url)
            )
        } else {
            format!(
                "IMPORT FUNCTION {} FROM {} WITH ({})",
                self.name,
                escape_string(&from_url),
                with_parts.join(", ")
            )
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for ImportFunctionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        if self.is_wildcard() {
            // Wildcard/bulk mode — discover all functions from manifest
            let namespace = self.wildcard_namespace();

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
            for entry in &manifest.functions {
                let input_types = entry
                    .input_types
                    .iter()
                    .map(|s| parse_arrow_type_name(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_type = parse_arrow_type_name(&entry.return_type)?;
                let kind: FunctionKind = entry.kind.parse()?;

                let symbol = entry.symbol.as_deref().unwrap_or(&entry.name);
                let logic = format!("{}:{}", self.logic, symbol);
                let name = format!("{}.{}", namespace, entry.name);

                let op = ImportFunctionOp::new(
                    name,
                    input_types,
                    return_type,
                    self.runner,
                    logic,
                    self.platform.clone(),
                    kind,
                );
                builder.apply_operation(op.into()).await?;
                count += 1;
            }

            Ok(format!(
                "Loaded {} function(s) from '{}'",
                count, self.logic
            ))
        } else {
            // Single function mode
            let (input_type_names, return_type_name, kind) = match (&self.input_types, &self.return_type) {
                (Some(input_types), Some(return_type)) => {
                    (input_types.clone(), return_type.clone(), self.kind)
                }
                _ => {
                    // Auto-detect from manifest
                    let namespaced = parse_function_name(&self.name)?;
                    let func_name = &namespaced.name;
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

            let op = ImportFunctionOp::new(
                self.name.clone(),
                input_types,
                return_type,
                self.runner,
                self.logic.clone(),
                self.platform.clone(),
                kind,
            );
            builder.apply_operation(op.into()).await?;

            Ok(format!("Loaded function: {}", self.name))
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_import_function() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ipc://./my_func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
                assert!(c.return_type.is_none());
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_with_platform() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ipc://./my_func' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_wildcard() {
        let input = "IMPORT FUNCTION acme.* FROM 'lib://./mylib.so'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "./mylib.so");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_wildcard_with_args() {
        let input = "IMPORT FUNCTION tools.* FROM 'ipc://./my_func' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "tools.*");
                assert!(c.is_wildcard());
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
                assert_eq!(c.platform.os, "linux");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_absolute_path() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ipc:///usr/bin/my_func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "/usr/bin/my_func");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_python() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'python://mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:func");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_roundtrip() {
        let cmd = ImportFunctionCommand::new_auto_detect(
            "acme.double_val",
            Runner::Ipc,
            "./my_func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT FUNCTION acme.double_val FROM 'ipc://./my_func'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_roundtrip_with_platform() {
        let cmd = ImportFunctionCommand::new_auto_detect(
            "acme.double_val",
            Runner::Ipc,
            "./my_func",
            "linux/amd64".parse().unwrap(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("WITH (platform = 'linux/amd64')"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_roundtrip_aggregate() {
        let cmd = ImportFunctionCommand::new_auto_detect(
            "acme.my_sum",
            Runner::Ipc,
            "./my_sum",
            Platform::any(),
            FunctionKind::Aggregate,
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("type = 'aggregate'"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.kind, FunctionKind::Aggregate);
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_wildcard_roundtrip() {
        let cmd = ImportFunctionCommand::new_auto_detect(
            "acme.*",
            Runner::Lib,
            "./mylib.so",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT FUNCTION acme.* FROM 'lib://./mylib.so'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "./mylib.so");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_case_insensitive() {
        let input = "load function acme.double_val from 'ipc://./test'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.runner, Runner::Ipc);
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_lib_with_symbol() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'lib://./mylib.so:double_val'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "./mylib.so:double_val");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }
}
