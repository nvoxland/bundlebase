//! Undo command implementation.

use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to undo the last uncommitted change(s).
#[derive(Debug, Clone)]
pub struct UndoCommand {
    /// Number of changes to undo (default 1).
    pub count: usize,
}

impl UndoCommand {
    /// Create a new UndoCommand that undoes `count` changes.
    pub fn new(count: usize) -> Self {
        Self { count }
    }
}

impl Default for UndoCommand {
    fn default() -> Self {
        Self { count: 1 }
    }
}

impl CommandParsing for UndoCommand {
    fn rule() -> Rule {
        Rule::undo_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut count = 1usize;
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::undo_count {
                count = inner
                    .as_str()
                    .parse::<usize>()
                    .map_err(|e| BundlebaseError::from(format!("Invalid UNDO count: {}", e)))?;
                if count == 0 {
                    return Err("UNDO count must be at least 1".into());
                }
            }
        }
        Ok(UndoCommand::new(count))
    }

    fn to_statement(&self) -> String {
        if self.count == 1 {
            "UNDO".to_string()
        } else {
            format!("UNDO LAST {}", self.count)
        }
    }
}

impl BundleBuilderCommand for UndoCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let available = builder.status().changes().len();
        if self.count > available {
            return Err(format!(
                "Cannot undo {} change{}: only {} uncommitted change{} available",
                self.count,
                if self.count == 1 { "" } else { "s" },
                available,
                if available == 1 { "" } else { "s" },
            )
            .into());
        }

        if self.count == 1 {
            let description = builder.undo().await?;
            Ok(format!("UNDONE: {}", description))
        } else {
            for _ in 0..self.count {
                builder.undo().await?;
            }
            Ok(format!("UNDONE: LAST {}", self.count))
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_undo() {
        let input = "UNDO";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Undo(c) => assert_eq!(c.count, 1),
            _ => panic!("Expected Undo variant"),
        }
    }

    #[test]
    fn test_parse_undo_last() {
        let input = "UNDO LAST 5";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Undo(c) => assert_eq!(c.count, 5),
            _ => panic!("Expected Undo variant"),
        }
    }

    #[test]
    fn test_parse_undo_last_case_insensitive() {
        let input = "undo last 3";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Undo(c) => assert_eq!(c.count, 3),
            _ => panic!("Expected Undo variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = UndoCommand::new(1);
        assert_eq!(cmd.to_statement(), "UNDO");

        let cmd = UndoCommand::new(3);
        assert_eq!(cmd.to_statement(), "UNDO LAST 3");
    }
}
