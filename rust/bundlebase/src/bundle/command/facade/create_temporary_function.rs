//! CreateTemporaryFunction command implementation (runtime-only).
//!
//! Creates a function with runtime-only logic, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::connector_definition::{Platform, Runner};
use crate::bundle::facade::BundleFacade;
use crate::bundle::function_definition::{parse_arrow_type_name, parse_function_name, FunctionEntry, FunctionKind};
use crate::function::lib_bridge::lookup_function_in_manifest;
use crate::NamespacedName;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Command to create a function with runtime-only logic (not persisted).
#[derive(Debug, Clone)]
pub struct CreateTemporaryFunctionCommand {
    /// Full dotted function name
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

impl CreateTemporaryFunctionCommand {
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

impl CommandParsing for CreateTemporaryFunctionCommand {
    fn rule() -> Rule {
        Rule::create_temporary_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut input_types = Vec::new();
        let mut return_type = None;
        let mut has_type_signature = false;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::function_params => {
                    has_type_signature = true;
                    for param_pair in inner_pair.into_inner() {
                        if param_pair.as_rule() == Rule::identifier {
                            input_types.push(param_pair.as_str().to_string());
                        }
                    }
                }
                Rule::identifier => {
                    return_type = Some(inner_pair.as_str().to_string());
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
            "CREATE TEMPORARY FUNCTION missing function name".into()
        })?;

        let runner_str = args.remove("runner").ok_or_else(|| -> BundlebaseError {
            "CREATE TEMPORARY FUNCTION requires 'runner' argument".into()
        })?;
        let runner: Runner = runner_str.parse()?;

        let logic = args.remove("logic").ok_or_else(|| -> BundlebaseError {
            "CREATE TEMPORARY FUNCTION requires 'logic' argument".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        let kind: FunctionKind = match args.remove("type") {
            Some(s) => s.parse()?,
            None => FunctionKind::Scalar,
        };

        if has_type_signature {
            let return_type = return_type.ok_or_else(|| -> BundlebaseError {
                "CREATE TEMPORARY FUNCTION has parameter types but missing RETURNS type".into()
            })?;
            Ok(CreateTemporaryFunctionCommand::new(name, input_types, return_type, runner, logic, platform, kind))
        } else {
            Ok(CreateTemporaryFunctionCommand::new_auto_detect(name, runner, logic, platform, kind))
        }
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        let runner_str = self.runner.to_string();
        let mut parts = vec![
            format!("runner = {}", escape_string(&runner_str)),
            format!("logic = {}", escape_string(&self.logic)),
            format!("platform = {}", escape_string(&self.platform.to_string())),
        ];
        if self.kind != FunctionKind::Scalar {
            parts.push(format!("type = {}", escape_string(&self.kind.to_string())));
        }
        let with_clause = parts.join(", ");

        match (&self.input_types, &self.return_type) {
            (Some(input_types), Some(return_type)) => {
                format!(
                    "CREATE TEMPORARY FUNCTION {}({}) RETURNS {} WITH ({})",
                    self.name,
                    input_types.join(", "),
                    return_type,
                    with_clause
                )
            }
            _ => {
                format!(
                    "CREATE TEMPORARY FUNCTION {} WITH ({})",
                    self.name,
                    with_clause
                )
            }
        }
    }
}

#[async_trait]
impl BundleFacadeCommand for CreateTemporaryFunctionCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let namespaced: NamespacedName = self.name.parse()
            .map_err(|e: crate::BundlebaseError| e)?;

        let (input_type_names, return_type_name, kind) = match (&self.input_types, &self.return_type) {
            (Some(input_types), Some(return_type)) => {
                (input_types.clone(), return_type.clone(), self.kind)
            }
            _ => {
                // Auto-detect from manifest
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
            name: namespaced,
            input_types,
            return_type,
            runner: self.runner,
            logic: self.logic.clone(),
            platform: self.platform.clone(),
            temporary: true,
            kind,
        };
        facade.create_temporary_function(entry).await?;
        Ok(format!("Created temporary function: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_temporary_function() {
        let input = "CREATE TEMPORARY FUNCTION acme.double_val(Int64) RETURNS Int64 WITH (runner = 'python', logic = 'mod:func')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
                assert_eq!(c.return_type, Some("Int64".to_string()));
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected CreateTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_temporary_function_aggregate() {
        let input = "CREATE TEMPORARY FUNCTION acme.my_sum(Int64) RETURNS Int64 WITH (runner = 'python', logic = 'mod:MySum', type = 'aggregate')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.my_sum");
                assert_eq!(c.kind, FunctionKind::Aggregate);
            }
            _ => panic!("Expected CreateTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_temporary_function_roundtrip() {
        let cmd = CreateTemporaryFunctionCommand::new(
            "acme.double_val",
            vec!["Int64".to_string()],
            "Int64",
            Runner::Python,
            "mod:func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
                assert_eq!(c.return_type, Some("Int64".to_string()));
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected CreateTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_default_platform() {
        let input = "CREATE TEMPORARY FUNCTION acme.double_val(Int64) RETURNS Int64 WITH (runner = 'python', logic = 'mod:func')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateTemporaryFunction(c) => {
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected CreateTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_temporary_function_without_types() {
        let input = "CREATE TEMPORARY FUNCTION acme.double_val WITH (runner = 'python', logic = 'mod:func')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
                assert!(c.return_type.is_none());
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:func");
            }
            _ => panic!("Expected CreateTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_to_statement_without_types_roundtrip() {
        let cmd = CreateTemporaryFunctionCommand::new_auto_detect(
            "acme.double_val",
            Runner::Python,
            "mod:func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        assert!(statement.starts_with("CREATE TEMPORARY FUNCTION acme.double_val WITH"));
        assert!(!statement.contains("RETURNS"));

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
                assert!(c.return_type.is_none());
            }
            _ => panic!("Expected CreateTemporaryFunction variant"),
        }
    }
}
