//! DetachBlock command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::DetachBlockOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to detach a data block from the bundle by its location.
#[derive(Debug, Clone)]
pub struct DetachBlockCommand {
    /// The location (URL) of the block to detach
    pub location: String,
}

impl DetachBlockCommand {
    /// Create a new DetachBlockCommand.
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
        }
    }
}

impl CommandParsing for DetachBlockCommand {
    fn rule() -> Rule {
        Rule::detach_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut location = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                location = Some(extract_string_content(inner.as_str())?);
            }
        }

        let location = location.ok_or_else(|| -> BundlebaseError {
            "DETACH statement missing location".into()
        })?;

        Ok(DetachBlockCommand::new(location))
    }

    fn to_statement(&self) -> String {
        format!("DETACH {}", escape_string(&self.location))
    }
}

#[async_trait]
impl BundleBuilderCommand for DetachBlockCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DetachBlockOp::setup(&self.location, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Detached {}", self.location))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_detach() {
        let input = "DETACH 's3://bucket/data.parquet'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DetachBlock(c) => {
                assert_eq!(c.location, "s3://bucket/data.parquet");
            }
            _ => panic!("Expected DetachBlock variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DetachBlockCommand::new("file:///data/test.csv");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DETACH 'file:///data/test.csv'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DetachBlock(c) => {
                assert_eq!(c.location, "file:///data/test.csv");
            }
            _ => panic!("Expected DetachBlock variant"),
        }
    }
}
