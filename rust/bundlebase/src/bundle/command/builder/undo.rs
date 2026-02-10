//! Undo command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to undo the last uncommitted change.
#[derive(Debug, Clone, Default)]
pub struct UndoCommand;

impl UndoCommand {
    /// Create a new UndoCommand.
    pub fn new() -> Self {
        Self
    }
}

impl CommandParsing for UndoCommand {
    fn rule() -> Rule {
        Rule::undo_stmt
    }

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(UndoCommand::new())
    }

    fn to_statement(&self) -> String {
        "UNDO".to_string()
    }
}

#[async_trait]
impl BundleBuilderCommand for UndoCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder.undo().await?;
        Ok("Undid last operation".to_string())
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_undo() {
        let input = "UNDO";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Undo(_) => {}
            _ => panic!("Expected Undo variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = UndoCommand::new();
        let statement = cmd.to_statement();
        assert_eq!(statement, "UNDO");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Undo(_) => {}
            _ => panic!("Expected Undo variant"),
        }
    }
}
