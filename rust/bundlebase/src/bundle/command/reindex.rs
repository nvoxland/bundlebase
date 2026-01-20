//! Reindex command implementation.

use crate::bundle::command::{Command, CommandContext, Rule};
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to rebuild all indexes.
#[derive(Debug, Clone, Default)]
pub struct ReindexCommand;

impl ReindexCommand {
    /// Create a new ReindexCommand.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Command for ReindexCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        ctx.reindex_internal().await
    }

    fn rule() -> Option<Rule> {
        Some(Rule::reindex_stmt)
    }

    fn from_pest(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // REINDEX has no parameters
        Ok(ReindexCommand::new())
    }

    fn to_statement(&self) -> String {
        "REINDEX".to_string()
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_reindex() {
        let input = "REINDEX";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Reindex(_) => {}
            _ => panic!("Expected Reindex variant"),
        }
    }

    #[test]
    fn test_parse_reindex_lowercase() {
        let input = "reindex";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Reindex(_) => {}
            _ => panic!("Expected Reindex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ReindexCommand::new();
        let statement = cmd.to_statement();
        assert_eq!(statement, "REINDEX");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Reindex(_) => {}
            _ => panic!("Expected Reindex variant"),
        }
    }
}
