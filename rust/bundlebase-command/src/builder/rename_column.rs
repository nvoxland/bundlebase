//! RenameColumn command implementation.

use crate::{CommandParsing, Rule};
use crate::parser::{extract_identifier, quote_identifier};
use bundlebase::bundle::operation::RenameColumnOp;
use bundlebase::bundle::BundleFacade;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to rename a column.
#[derive(Debug, Clone)]
pub struct RenameColumnCommand {
    /// The current column name
    pub old_name: String,
    /// The new column name
    pub new_name: String,
}

impl RenameColumnCommand {
    /// Create a new RenameColumnCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

impl CommandParsing for RenameColumnCommand {
    fn rule() -> Rule {
        Rule::rename_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_name = None;
        let mut new_name = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                if old_name.is_none() {
                    old_name = Some(extract_identifier(&inner));
                } else {
                    new_name = Some(extract_identifier(&inner));
                }
            }
        }

        let old_name = old_name.ok_or_else(|| -> BundlebaseError {
            "RENAME COLUMN statement missing old column name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME COLUMN statement missing new column name".into()
        })?;

        Ok(RenameColumnCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!(
            "RENAME COLUMN {} TO {}",
            quote_identifier(&self.old_name),
            quote_identifier(&self.new_name)
        )
    }
}

impl BundleBuilderCommand for RenameColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let column_id = builder.column_id(&self.old_name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.old_name)))?;

        builder
            .apply_operation(
                RenameColumnOp::setup(column_id, &self.new_name).into(),
            )
            .await?;
        Ok(format!("Renamed column: {} to {}", self.old_name, self.new_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_rename_column() {
        let input = "RENAME COLUMN old_name TO new_name";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "old_name");
                assert_eq!(c.new_name, "new_name");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RenameColumnCommand::new("user_id", "customer_id");
        let statement = cmd.to_statement();
        assert_eq!(statement, "RENAME COLUMN user_id TO customer_id");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "user_id");
                assert_eq!(c.new_name, "customer_id");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }

    #[test]
    fn test_parse_quoted_source() {
        let input = r#"RENAME COLUMN "ResultMeasureValue" TO secchi_depth"#;
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "ResultMeasureValue");
                assert_eq!(c.new_name, "secchi_depth");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }

    #[test]
    fn test_parse_quoted_target() {
        let input = r#"RENAME COLUMN old_name TO "new name with spaces""#;
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "old_name");
                assert_eq!(c.new_name, "new name with spaces");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }

    #[test]
    fn test_parse_both_quoted() {
        let input = r#"RENAME COLUMN "old/name" TO "new.name""#;
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "old/name");
                assert_eq!(c.new_name, "new.name");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }

    #[test]
    fn test_round_trip_quoted() {
        let cmd = RenameColumnCommand::new("column with spaces", "new.name");
        let statement = cmd.to_statement();
        assert_eq!(statement, r#"RENAME COLUMN "column with spaces" TO "new.name""#);

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameColumn(c) => {
                assert_eq!(c.old_name, "column with spaces");
                assert_eq!(c.new_name, "new.name");
            }
            _ => panic!("Expected RenameColumn variant"),
        }
    }
}
