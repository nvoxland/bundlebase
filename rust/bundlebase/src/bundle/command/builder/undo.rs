//! Undo command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::BundlebaseError;
use async_trait::async_trait;
use log::info;
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
    type Output = ();

    async fn execute(self: Box<Self>, builder: &mut BundleBuilder) -> Result<(), BundlebaseError> {
        if builder.status().is_empty() {
            return Err("No uncommitted changes to undo".into());
        }

        // Remove the last change
        builder.status.pop();

        // Reload the bundle from the last committed state
        builder.reload_bundle().await?;

        // Reapply all remaining operations
        let changes = builder.status.changes().clone();
        for change in &changes {
            for op in &change.operations {
                builder.bundle_mut().apply_operation(op.clone()).await?;
            }
        }

        info!("Last operation undone");

        Ok(())
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
