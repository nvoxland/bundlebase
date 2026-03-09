//! CreateFunctionsFrom command implementation — bulk function discovery and registration.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::connector_definition::{Platform, Runner};
use crate::bundle::function_definition::{parse_arrow_type_name, FunctionKind};
use crate::bundle::operation::CreateFunctionOp;
use crate::function::lib_bridge::{load_ipc_manifest, load_lib_manifest};
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to discover and register all functions from a shared library or IPC executable.
///
/// Uses the manifest discovery protocol:
/// - **Lib**: calls `bundlebase_functions()` C symbol
/// - **IPC**: runs `path --bundlebase-functions`
#[derive(Debug, Clone)]
pub struct CreateFunctionsFromCommand {
    /// Path to the shared library or IPC executable
    pub path: String,
    /// Runner type (lib or ipc)
    pub runner: Runner,
    /// Namespace for the registered functions
    pub namespace: String,
    /// Platform
    pub platform: Platform,
}

impl CreateFunctionsFromCommand {
    pub fn new(
        path: impl Into<String>,
        runner: Runner,
        namespace: impl Into<String>,
        platform: Platform,
    ) -> Self {
        Self {
            path: path.into(),
            runner,
            namespace: namespace.into(),
            platform,
        }
    }
}

impl CommandParsing for CreateFunctionsFromCommand {
    fn rule() -> Rule {
        Rule::create_functions_from_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::quoted_string => {
                    path = Some(extract_string_content(inner_pair.as_str())?);
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

        let path = path.ok_or_else(|| -> BundlebaseError {
            "CREATE FUNCTIONS FROM missing path".into()
        })?;

        let runner_str = args.remove("runner").ok_or_else(|| -> BundlebaseError {
            "CREATE FUNCTIONS FROM requires 'runner' argument".into()
        })?;
        let runner: Runner = runner_str.parse()?;

        let namespace = args.remove("namespace").ok_or_else(|| -> BundlebaseError {
            "CREATE FUNCTIONS FROM requires 'namespace' argument".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(CreateFunctionsFromCommand::new(path, runner, namespace, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        let mut parts = vec![
            format!("runner = {}", escape_string(&self.runner.to_string())),
            format!("namespace = {}", escape_string(&self.namespace)),
        ];
        if self.platform != Platform::any() {
            parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }
        format!(
            "CREATE FUNCTIONS FROM {} WITH ({})",
            escape_string(&self.path),
            parts.join(", ")
        )
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateFunctionsFromCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let manifest = match self.runner {
            Runner::Lib => load_lib_manifest(&self.path)?,
            Runner::Ipc => load_ipc_manifest(&self.path)?,
            other => {
                return Err(format!(
                    "CREATE FUNCTIONS FROM only supports 'lib' and 'ipc' runners, got '{}'",
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

            // Build the logic string: path:symbol
            let symbol = entry.symbol.as_deref().unwrap_or(&entry.name);
            let logic = format!("{}:{}", self.path, symbol);

            let name = format!("{}.{}", self.namespace, entry.name);

            let op = CreateFunctionOp::new(
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
            "Created {} function(s) from '{}'",
            count, self.path
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_functions_from_lib() {
        let input = "CREATE FUNCTIONS FROM './mylib.so' WITH (runner = 'lib', namespace = 'acme')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunctionsFrom(c) => {
                assert_eq!(c.path, "./mylib.so");
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.namespace, "acme");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected CreateFunctionsFrom variant"),
        }
    }

    #[test]
    fn test_parse_create_functions_from_ipc() {
        let input = "CREATE FUNCTIONS FROM './my_func' WITH (runner = 'ipc', namespace = 'tools')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunctionsFrom(c) => {
                assert_eq!(c.path, "./my_func");
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.namespace, "tools");
            }
            _ => panic!("Expected CreateFunctionsFrom variant"),
        }
    }

    #[test]
    fn test_parse_create_functions_from_with_platform() {
        let input = "CREATE FUNCTIONS FROM './mylib.so' WITH (runner = 'lib', namespace = 'acme', platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunctionsFrom(c) => {
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected CreateFunctionsFrom variant"),
        }
    }

    #[test]
    fn test_parse_create_functions_from_case_insensitive() {
        let input = "create functions from './mylib.so' with (runner = 'lib', namespace = 'acme')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunctionsFrom(_) => {}
            _ => panic!("Expected CreateFunctionsFrom variant"),
        }
    }

    #[test]
    fn test_parse_create_functions_from_roundtrip() {
        let cmd = CreateFunctionsFromCommand::new(
            "./mylib.so",
            Runner::Lib,
            "acme",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateFunctionsFrom(c) => {
                assert_eq!(c.path, "./mylib.so");
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.namespace, "acme");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected CreateFunctionsFrom variant"),
        }
    }

    #[test]
    fn test_parse_create_functions_from_roundtrip_with_platform() {
        let cmd = CreateFunctionsFromCommand::new(
            "./mylib.so",
            Runner::Lib,
            "acme",
            "linux/amd64".parse().unwrap(),
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("platform = 'linux/amd64'"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateFunctionsFrom(c) => {
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected CreateFunctionsFrom variant"),
        }
    }
}
