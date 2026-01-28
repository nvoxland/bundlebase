//! ReplaceBlock command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::ReplaceBlockOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to replace a block's location in the bundle.
#[derive(Debug, Clone)]
pub struct ReplaceBlockCommand {
    /// The current location (URL) of the block
    pub old_location: String,
    /// The new location (URL) to read data from
    pub new_location: String,
}

impl ReplaceBlockCommand {
    /// Create a new ReplaceBlockCommand.
    pub fn new(old_location: impl Into<String>, new_location: impl Into<String>) -> Self {
        Self {
            old_location: old_location.into(),
            new_location: new_location.into(),
        }
    }
}

impl CommandParsing for ReplaceBlockCommand {
    fn rule() -> Rule {
        Rule::replace_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_location = None;
        let mut new_location = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                if old_location.is_none() {
                    old_location = Some(extract_string_content(inner.as_str())?);
                } else if new_location.is_none() {
                    new_location = Some(extract_string_content(inner.as_str())?);
                }
            }
        }

        let old_location = old_location.ok_or_else(|| -> BundlebaseError {
            "REPLACE statement missing old location".into()
        })?;
        let new_location = new_location.ok_or_else(|| -> BundlebaseError {
            "REPLACE statement missing new location".into()
        })?;

        Ok(ReplaceBlockCommand::new(old_location, new_location))
    }

    fn to_statement(&self) -> String {
        format!(
            "REPLACE {} WITH {}",
            escape_string(&self.old_location),
            escape_string(&self.new_location)
        )
    }
}

#[async_trait]
impl BundleBuilderCommand for ReplaceBlockCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = ReplaceBlockOp::setup(&self.old_location, &self.new_location, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Replaced {} with {}", self.old_location, self.new_location))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_replace() {
        let input = "REPLACE 's3://old/data.parquet' WITH 's3://new/data.parquet'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ReplaceBlock(c) => {
                assert_eq!(c.old_location, "s3://old/data.parquet");
                assert_eq!(c.new_location, "s3://new/data.parquet");
            }
            _ => panic!("Expected ReplaceBlock variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ReplaceBlockCommand::new("file:///old.csv", "file:///new.csv");
        let statement = cmd.to_statement();
        assert_eq!(statement, "REPLACE 'file:///old.csv' WITH 'file:///new.csv'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ReplaceBlock(c) => {
                assert_eq!(c.old_location, "file:///old.csv");
                assert_eq!(c.new_location, "file:///new.csv");
            }
            _ => panic!("Expected ReplaceBlock variant"),
        }
    }
}
