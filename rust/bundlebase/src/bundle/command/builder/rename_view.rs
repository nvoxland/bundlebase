//! RenameView command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::RenameViewOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to rename a view.
#[derive(Debug, Clone)]
pub struct RenameViewCommand {
    /// The current view name
    pub old_name: String,
    /// The new view name
    pub new_name: String,
}

impl RenameViewCommand {
    /// Create a new RenameViewCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

impl CommandParsing for RenameViewCommand {
    fn rule() -> Rule {
        Rule::rename_view_stmt
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
            "RENAME VIEW statement missing old name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME VIEW statement missing new name".into()
        })?;

        Ok(RenameViewCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!("RENAME VIEW {} TO {}", self.old_name, self.new_name)
    }
}

#[async_trait]
impl BundleBuilderCommand for RenameViewCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = RenameViewOp::setup(&self.old_name, &self.new_name, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Renamed view: {} to {}", self.old_name, self.new_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_rename_view() {
        let input = "RENAME VIEW old_view TO new_view";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameView(c) => {
                assert_eq!(c.old_name, "old_view");
                assert_eq!(c.new_name, "new_view");
            }
            _ => panic!("Expected RenameView variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RenameViewCommand::new("summary", "detailed_summary");
        let statement = cmd.to_statement();
        assert_eq!(statement, "RENAME VIEW summary TO detailed_summary");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameView(c) => {
                assert_eq!(c.old_name, "summary");
                assert_eq!(c.new_name, "detailed_summary");
            }
            _ => panic!("Expected RenameView variant"),
        }
    }
}
