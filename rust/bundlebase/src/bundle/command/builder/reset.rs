//! Reset command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to reset all uncommitted changes.
#[derive(Debug, Clone, Default)]
pub struct ResetCommand;

impl ResetCommand {
    /// Create a new ResetCommand.
    pub fn new() -> Self {
        Self
    }
}

impl CommandParsing for ResetCommand {
    fn rule() -> Rule {
        Rule::reset_stmt
    }

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(ResetCommand::new())
    }

    fn to_statement(&self) -> String {
        "RESET".to_string()
    }
}

#[async_trait]
impl BundleBuilderCommand for ResetCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder.reset().await?;
        Ok("Reset to last committed state".to_string())
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_reset() {
        let input = "RESET";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Reset(_) => {}
            _ => panic!("Expected Reset variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ResetCommand::new();
        let statement = cmd.to_statement();
        assert_eq!(statement, "RESET");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Reset(_) => {}
            _ => panic!("Expected Reset variant"),
        }
    }
}
