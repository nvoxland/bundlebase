//! Commit command implementation.

use crate::parser::extract_string_content;
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

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

        let message = message
            .ok_or_else(|| -> BundlebaseError { "COMMIT statement missing message".into() })?;

        Ok(CommitCommand::new(message))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        format!("COMMIT {}", escape_string(&self.message))
    }
}

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
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

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

    #[test]
    fn test_parse_dollar_quoted_commit() {
        let input = "COMMIT $$it's done$$";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Commit(c) => {
                assert_eq!(c.message, "it's done");
            }
            _ => panic!("Expected Commit variant"),
        }
    }

    #[test]
    fn test_dollar_quoted_round_trip_with_single_quotes() {
        // Message contains a single quote — to_statement() should use $$...$$
        let cmd = CommitCommand::new("it's a fix");
        let statement = cmd.to_statement();
        assert_eq!(statement, "COMMIT $$it's a fix$$");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Commit(c) => {
                assert_eq!(c.message, "it's a fix");
            }
            _ => panic!("Expected Commit variant"),
        }
    }

    #[test]
    fn test_dollar_quoted_multiline() {
        let input = "COMMIT $$line1\nline2$$";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Commit(c) => {
                assert_eq!(c.message, "line1\nline2");
            }
            _ => panic!("Expected Commit variant"),
        }
    }
}
