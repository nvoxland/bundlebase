//! RenameFunction command implementation.

use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::RenameFunctionOp;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to rename a function.
///
/// Renames all entries for a function name to a new dotted name.
/// Deregisters old UDFs and re-registers under the new name.
#[derive(Debug, Clone)]
pub struct RenameFunctionCommand {
    /// The current function name (dotted, e.g. "acme.double_val")
    pub old_name: String,
    /// The new function name (dotted, e.g. "acme.double_val_v2")
    pub new_name: String,
}

impl RenameFunctionCommand {
    /// Create a new RenameFunctionCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

impl CommandParsing for RenameFunctionCommand {
    fn rule() -> Rule {
        Rule::rename_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_name = None;
        let mut new_name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                if old_name.is_none() {
                    old_name = Some(inner_pair.as_str().to_string());
                } else {
                    new_name = Some(inner_pair.as_str().to_string());
                }
            }
        }

        let old_name = old_name.ok_or_else(|| -> BundlebaseError {
            "RENAME FUNCTION statement missing old name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME FUNCTION statement missing new name".into()
        })?;

        Ok(RenameFunctionCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!("RENAME FUNCTION {} TO {}", self.old_name, self.new_name)
    }
}

impl BundleBuilderCommand for RenameFunctionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = RenameFunctionOp::setup(&self.old_name, &self.new_name, builder)?;
        builder.apply_operation(op.into()).await?;
        Ok(format!(
            "Renamed function: {} to {}",
            self.old_name, self.new_name
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_rename_function() {
        let input = "RENAME FUNCTION acme.double_val TO acme.double_val_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameFunction(c) => {
                assert_eq!(c.old_name, "acme.double_val");
                assert_eq!(c.new_name, "acme.double_val_v2");
            }
            _ => panic!("Expected RenameFunction variant"),
        }
    }

    #[test]
    fn test_parse_rename_function_case_insensitive() {
        let input = "rename function acme.double_val to acme.double_val_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameFunction(c) => {
                assert_eq!(c.old_name, "acme.double_val");
                assert_eq!(c.new_name, "acme.double_val_v2");
            }
            _ => panic!("Expected RenameFunction variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RenameFunctionCommand::new("acme.double_val", "acme.double_val_v2");
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "RENAME FUNCTION acme.double_val TO acme.double_val_v2"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameFunction(c) => {
                assert_eq!(c.old_name, "acme.double_val");
                assert_eq!(c.new_name, "acme.double_val_v2");
            }
            _ => panic!("Expected RenameFunction variant"),
        }
    }
}
