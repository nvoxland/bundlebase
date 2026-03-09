//! CreateFunction command implementation (persistent).

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::connector_definition::{Platform, Runner};
use crate::bundle::function_definition::{parse_arrow_type_name, parse_function_name, FunctionKind};
use crate::bundle::operation::CreateFunctionOp;
use crate::function::lib_bridge::lookup_function_in_manifest;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to define a named function with its logic.
#[derive(Debug, Clone)]
pub struct CreateFunctionCommand {
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

impl CreateFunctionCommand {
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
}

impl CommandParsing for CreateFunctionCommand {
    fn rule() -> Rule {
        Rule::create_function_stmt
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
                    // This is the RETURNS type
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
            "CREATE FUNCTION missing function name".into()
        })?;

        let runner_str = args.remove("runner").ok_or_else(|| -> BundlebaseError {
            "CREATE FUNCTION requires 'runner' argument".into()
        })?;
        let runner: Runner = runner_str.parse()?;

        let logic = args.remove("logic").ok_or_else(|| -> BundlebaseError {
            "CREATE FUNCTION requires 'logic' argument".into()
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
                "CREATE FUNCTION has parameter types but missing RETURNS type".into()
            })?;
            Ok(CreateFunctionCommand::new(name, input_types, return_type, runner, logic, platform, kind))
        } else {
            Ok(CreateFunctionCommand::new_auto_detect(name, runner, logic, platform, kind))
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
                    "CREATE FUNCTION {}({}) RETURNS {} WITH ({})",
                    self.name,
                    input_types.join(", "),
                    return_type,
                    with_clause
                )
            }
            _ => {
                format!(
                    "CREATE FUNCTION {} WITH ({})",
                    self.name,
                    with_clause
                )
            }
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateFunctionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
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

        let op = CreateFunctionOp::new(
            self.name.clone(),
            input_types,
            return_type,
            self.runner,
            self.logic.clone(),
            self.platform.clone(),
            kind,
        );
        builder.apply_operation(op.into()).await?;

        Ok(format!("Created function: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_function() {
        let input = "CREATE FUNCTION acme.double_val(Int64) RETURNS Int64 WITH (runner = 'ipc', logic = './my_func')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
                assert_eq!(c.return_type, Some("Int64".to_string()));
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_aggregate() {
        let input = "CREATE FUNCTION acme.my_sum(Int64) RETURNS Int64 WITH (runner = 'ipc', logic = './my_sum', type = 'aggregate')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.my_sum");
                assert_eq!(c.kind, FunctionKind::Aggregate);
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_multi_arg() {
        let input = "CREATE FUNCTION acme.add(Int64, Int64) RETURNS Int64 WITH (runner = 'ipc', logic = './add')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.add");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string(), "Int64".to_string()]));
                assert_eq!(c.return_type, Some("Int64".to_string()));
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_roundtrip() {
        let cmd = CreateFunctionCommand::new(
            "acme.double_val",
            vec!["Int64".to_string()],
            "Int64",
            Runner::Ipc,
            "./my_func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
                assert_eq!(c.return_type, Some("Int64".to_string()));
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
                assert_eq!(c.platform, Platform::any());
                assert_eq!(c.kind, FunctionKind::Scalar);
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_roundtrip_aggregate() {
        let cmd = CreateFunctionCommand::new(
            "acme.my_sum",
            vec!["Int64".to_string()],
            "Int64",
            Runner::Ipc,
            "./my_sum",
            Platform::any(),
            FunctionKind::Aggregate,
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("type = 'aggregate'"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.kind, FunctionKind::Aggregate);
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_case_insensitive() {
        let input = "create function acme.double_val(Int64) returns Int64 with (runner = 'ipc', logic = './test')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.runner, Runner::Ipc);
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_without_types() {
        let input = "CREATE FUNCTION acme.double_val WITH (runner = 'ipc', logic = './my_func')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
                assert!(c.return_type.is_none());
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_parse_create_function_with_types_still_works() {
        let input = "CREATE FUNCTION acme.double_val(Int64) RETURNS Int64 WITH (runner = 'ipc', logic = './my_func')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
                assert_eq!(c.return_type, Some("Int64".to_string()));
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }

    #[test]
    fn test_to_statement_without_types() {
        let cmd = CreateFunctionCommand::new_auto_detect(
            "acme.double_val",
            Runner::Ipc,
            "./my_func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        assert!(statement.starts_with("CREATE FUNCTION acme.double_val WITH"));
        assert!(!statement.contains("RETURNS"));
        assert!(!statement.contains("()"));
    }

    #[test]
    fn test_to_statement_without_types_roundtrip() {
        let cmd = CreateFunctionCommand::new_auto_detect(
            "acme.double_val",
            Runner::Ipc,
            "./my_func",
            Platform::any(),
            FunctionKind::Scalar,
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
                assert!(c.return_type.is_none());
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_func");
            }
            _ => panic!("Expected CreateFunction variant"),
        }
    }
}
