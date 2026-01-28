//! RenameColumn command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::RenameColumnOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to rename a column.
#[derive(Debug, Clone)]
pub struct RenameColumnCommand {
    /// The current column name
    pub old_name: String,
    /// The new column name
    pub new_name: String,
}

impl RenameColumnCommand {
    /// Create a new RenameColumnCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

impl CommandParsing for RenameColumnCommand {
    fn rule() -> Rule {
        Rule::rename_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_name = None;
        let mut new_name = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                if old_name.is_none() {
                    old_name = Some(inner.as_str().to_string());
                } else {
                    new_name = Some(inner.as_str().to_string());
                }
            }
        }

        let old_name = old_name.ok_or_else(|| -> BundlebaseError {
            "RENAME COLUMN statement missing old column name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME COLUMN statement missing new column name".into()
        })?;

        Ok(RenameColumnCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!("RENAME COLUMN {} TO {}", self.old_name, self.new_name)
    }
}

#[async_trait]
impl BundleBuilderCommand for RenameColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(RenameColumnOp::setup(&self.old_name, &self.new_name).into())
            .await?;
        Ok(format!("Renamed column: {} to {}", self.old_name, self.new_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_rename_column() {
        let input = "RENAME COLUMN old_name TO new_name";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "old_name");
                assert_eq!(c.new_name, "new_name");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RenameColumnCommand::new("user_id", "customer_id");
        let statement = cmd.to_statement();
        assert_eq!(statement, "RENAME COLUMN user_id TO customer_id");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "user_id");
                assert_eq!(c.new_name, "customer_id");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }
}
