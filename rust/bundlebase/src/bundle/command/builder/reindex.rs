//! Reindex command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to rebuild all indexes.
#[derive(Debug, Clone, Default)]
pub struct ReindexCommand;

impl ReindexCommand {
    /// Create a new ReindexCommand.
    pub fn new() -> Self {
        Self
    }
}

impl CommandParsing for ReindexCommand {
    fn rule() -> Rule {
        Rule::reindex_stmt
    }

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // REINDEX has no parameters
        Ok(ReindexCommand::new())
    }

    fn to_statement(&self) -> String {
        "REINDEX".to_string()
    }
}

#[async_trait]
impl BundleBuilderCommand for ReindexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder.reindex_internal().await?;
        Ok("Rebuilt all indexes".to_string())
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
