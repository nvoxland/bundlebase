//! DropFunction command implementation (persistent).

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::connector_definition::Platform;
use crate::bundle::operation::DropFunctionOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop all overloads of a function by name.
#[derive(Debug, Clone)]
pub struct DropFunctionCommand {
    /// Full dotted function name
    pub name: String,
    /// Optional platform filter
    pub platform: Option<Platform>,
}

impl DropFunctionCommand {
    pub fn new(name: impl Into<String>, platform: Option<Platform>) -> Self {
        Self {
            name: name.into(),
            platform,
        }
    }
}

impl CommandParsing for DropFunctionCommand {
    fn rule() -> Rule {
        Rule::drop_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                name = Some(inner_pair.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP FUNCTION missing function name".into()
        })?;

        Ok(DropFunctionCommand::new(name, None))
    }

    fn to_statement(&self) -> String {
        format!("DROP FUNCTION {}", self.name)
    }
}

#[async_trait]
impl BundleBuilderCommand for DropFunctionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropFunctionOp::new_with_signature(
            self.name.clone(),
            self.platform.clone(),
            None,
        );
        builder.apply_operation(op.into()).await?;

        Ok(format!("Dropped function: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_function() {
        let input = "DROP FUNCTION acme.double_val";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_function_roundtrip() {
        let cmd = DropFunctionCommand::new("acme.double_val", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP FUNCTION acme.double_val");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }
}
