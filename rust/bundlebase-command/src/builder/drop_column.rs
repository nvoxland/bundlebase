//! DropColumn command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::DropColumnOp;
use bundlebase::bundle::BundleFacade;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to drop a column from the bundle.
#[derive(Debug, Clone)]
pub struct DropColumnCommand {
    /// The column name to drop
    pub name: String,
}

impl DropColumnCommand {
    /// Create a new DropColumnCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for DropColumnCommand {
    fn rule() -> Rule {
        Rule::drop_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                name = Some(extract_identifier(&inner));
            }
        }

        let name =
            name.ok_or_else(|| -> BundlebaseError { "DROP COLUMN missing column name".into() })?;

        Ok(DropColumnCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP COLUMN {}", quote_identifier(&self.name))
    }
}

impl BundleBuilderCommand for DropColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let column_id = builder.column_id(&self.name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.name)))?;

        builder
            .apply_operation(
                DropColumnOp::setup(column_id).into(),
            )
            .await?;

        Ok(format!("Dropped column: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_drop_column() {
        let input = "DROP COLUMN old_column";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropColumn(c) => {
                assert_eq!(c.name, "old_column");
            }
            _ => panic!("Expected DropColumn variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropColumnCommand::new("temp_col");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP COLUMN temp_col");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropColumn(c) => {
                assert_eq!(c.name, "temp_col");
            }
            _ => panic!("Expected DropColumn variant"),
        }
    }

    #[test]
    fn test_parse_quoted_identifier() {
        let input = r#"DROP COLUMN "weird/column.name""#;
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropColumn(c) => {
                assert_eq!(c.name, "weird/column.name");
            }
            _ => panic!("Expected DropColumn variant"),
        }
    }

    #[test]
    fn test_round_trip_quoted() {
        let cmd = DropColumnCommand::new("column with spaces");
        let statement = cmd.to_statement();
        assert_eq!(statement, r#"DROP COLUMN "column with spaces""#);

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropColumn(c) => {
                assert_eq!(c.name, "column with spaces");
            }
            _ => panic!("Expected DropColumn variant"),
        }
    }
}
