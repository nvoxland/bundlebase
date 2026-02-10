//! Commit command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::extract_string_content;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to commit changes.
///
/// The commit logic lives in `BundleBuilder::commit()`. This command
/// provides the parsing/serialization interface and delegates to that method.
#[derive(Debug, Clone)]
pub struct CommitCommand {
    /// The commit message
    pub message: String,
}

impl CommitCommand {
    /// Create a new CommitCommand.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl CommandParsing for CommitCommand {
    fn rule() -> Rule {
        Rule::commit_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut message = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                message = Some(extract_string_content(inner.as_str())?);
            }
        }

        let message = message.ok_or_else(|| -> BundlebaseError {
            "COMMIT statement missing message".into()
        })?;

        Ok(CommitCommand::new(message))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        format!("COMMIT {}", escape_string(&self.message))
    }
}

#[async_trait]
impl BundleBuilderCommand for CommitCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Commit is special - we need to call the builder's commit method directly
        // This will commit all pending changes (including any that were just added)
        builder.commit(&self.message).await?;
        Ok(format!("Committed: {}", self.message))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_commit() {
        let input = "COMMIT 'Added new data'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Commit(c) => {
                assert_eq!(c.message, "Added new data");
            }
            _ => panic!("Expected Commit variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = CommitCommand::new("Test commit message");
        let statement = cmd.to_statement();
        assert_eq!(statement, "COMMIT 'Test commit message'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Commit(c) => {
                assert_eq!(c.message, "Test commit message");
            }
            _ => panic!("Expected Commit variant"),
        }
    }
}
