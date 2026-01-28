//! RenameJoin command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::RenameJoinOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to rename a join.
#[derive(Debug, Clone)]
pub struct RenameJoinCommand {
    /// The current join name
    pub old_name: String,
    /// The new join name
    pub new_name: String,
}

impl RenameJoinCommand {
    /// Create a new RenameJoinCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

impl CommandParsing for RenameJoinCommand {
    fn rule() -> Rule {
        Rule::rename_join_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_name = None;
        let mut new_name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::identifier {
                if old_name.is_none() {
                    old_name = Some(inner_pair.as_str().to_string());
                } else {
                    new_name = Some(inner_pair.as_str().to_string());
                }
            }
        }

        let old_name = old_name.ok_or_else(|| -> BundlebaseError {
            "RENAME JOIN statement missing old join name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME JOIN statement missing new join name".into()
        })?;

        Ok(RenameJoinCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!("RENAME JOIN {} TO {}", self.old_name, self.new_name)
    }
}

#[async_trait]
impl BundleBuilderCommand for RenameJoinCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = RenameJoinOp::setup(&self.old_name, &self.new_name, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Renamed join: {} to {}", self.old_name, self.new_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_rename_join() {
        let input = "RENAME JOIN customers TO clients";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameJoin(c) => {
                assert_eq!(c.old_name, "customers");
                assert_eq!(c.new_name, "clients");
            }
            _ => panic!("Expected RenameJoin variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RenameJoinCommand::new("old_join", "new_join");
        let statement = cmd.to_statement();
        assert_eq!(statement, "RENAME JOIN old_join TO new_join");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameJoin(c) => {
                assert_eq!(c.old_name, "old_join");
                assert_eq!(c.new_name, "new_join");
            }
            _ => panic!("Expected RenameJoin variant"),
        }
    }
}
