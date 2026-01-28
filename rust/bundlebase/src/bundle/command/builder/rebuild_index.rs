//! RebuildIndex command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::RebuildIndexOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to rebuild an index on a column.
#[derive(Debug, Clone)]
pub struct RebuildIndexCommand {
    /// The column name to rebuild the index for
    pub column: String,
}

impl RebuildIndexCommand {
    /// Create a new RebuildIndexCommand.
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
        }
    }
}

impl CommandParsing for RebuildIndexCommand {
    fn rule() -> Rule {
        Rule::rebuild_index_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut column = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                column = Some(inner.as_str().to_string());
            }
        }

        let column = column.ok_or_else(|| -> BundlebaseError {
            "REBUILD INDEX statement missing column name".into()
        })?;

        Ok(RebuildIndexCommand::new(column))
    }

    fn to_statement(&self) -> String {
        format!("REBUILD INDEX ON {}", self.column)
    }
}

#[async_trait]
impl BundleBuilderCommand for RebuildIndexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = RebuildIndexOp::setup(self.column.clone()).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Rebuilt index on column: {}", self.column))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_rebuild_index() {
        let input = "REBUILD INDEX ON user_id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RebuildIndex(c) => {
                assert_eq!(c.column, "user_id");
            }
            _ => panic!("Expected RebuildIndex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RebuildIndexCommand::new("email");
        let statement = cmd.to_statement();
        assert_eq!(statement, "REBUILD INDEX ON email");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RebuildIndex(c) => {
                assert_eq!(c.column, "email");
            }
            _ => panic!("Expected RebuildIndex variant"),
        }
    }
}
