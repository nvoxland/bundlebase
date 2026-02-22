//! CastColumn command implementation.

use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::CastColumnOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to cast a column to a different data type.
#[derive(Debug, Clone)]
pub struct CastColumnCommand {
    /// The column name to cast
    pub column_name: String,
    /// The target type (e.g., "integer", "float", "string")
    pub new_type: String,
    /// Optional regex pattern to clean the column values before casting
    pub clean: Option<String>,
}

impl CastColumnCommand {
    /// Create a new CastColumnCommand.
    pub fn new(
        column_name: impl Into<String>,
        new_type: impl Into<String>,
        clean: Option<String>,
    ) -> Self {
        Self {
            column_name: column_name.into(),
            new_type: new_type.into(),
            clean,
        }
    }
}

impl CommandParsing for CastColumnCommand {
    fn rule() -> Rule {
        Rule::cast_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut column_name = None;
        let mut new_type = None;
        let mut clean = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    if column_name.is_none() {
                        column_name = Some(inner.as_str().to_string());
                    } else {
                        new_type = Some(inner.as_str().to_string());
                    }
                }
                Rule::quoted_string => {
                    clean = Some(extract_string_content(inner.as_str())?);
                }
                _ => {}
            }
        }

        let column_name = column_name.ok_or_else(|| -> BundlebaseError {
            "CAST COLUMN statement missing column name".into()
        })?;
        let new_type = new_type.ok_or_else(|| -> BundlebaseError {
            "CAST COLUMN statement missing target type".into()
        })?;

        Ok(CastColumnCommand::new(column_name, new_type, clean))
    }

    fn to_statement(&self) -> String {
        match &self.clean {
            Some(pattern) => format!(
                "CAST COLUMN {} TO {} CLEAN {}",
                self.column_name, self.new_type, escape_string(pattern)
            ),
            None => format!("CAST COLUMN {} TO {}", self.column_name, self.new_type),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for CastColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(
                CastColumnOp::setup(&self.column_name, &self.new_type, self.clean.clone())
                    .into(),
            )
            .await?;

        match &self.clean {
            Some(pattern) => Ok(format!(
                "Cast column {} to {} (clean: {})",
                self.column_name, self.new_type, pattern
            )),
            None => Ok(format!(
                "Cast column {} to {}",
                self.column_name, self.new_type
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::{BundleCommand, CommandParsing};

    #[test]
    fn test_parse_cast_column() {
        let cmd = parse_command("CAST COLUMN price TO integer").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.column_name, "price");
                assert_eq!(c.new_type, "integer");
                assert_eq!(c.clean, None);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_with_clean() {
        let cmd = parse_command("CAST COLUMN price TO integer CLEAN '[^0-9]'").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.column_name, "price");
                assert_eq!(c.new_type, "integer");
                assert_eq!(c.clean, Some("[^0-9]".to_string()));
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_case_insensitive() {
        let cmd = parse_command("cast column value to float").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.column_name, "value");
                assert_eq!(c.new_type, "float");
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = super::CastColumnCommand::new("price", "integer", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "CAST COLUMN price TO integer");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.column_name, "price");
                assert_eq!(c.new_type, "integer");
                assert_eq!(c.clean, None);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_with_clean() {
        let cmd = super::CastColumnCommand::new("price", "integer", Some("[^0-9]".to_string()));
        let statement = cmd.to_statement();

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.column_name, "price");
                assert_eq!(c.new_type, "integer");
                assert_eq!(c.clean, Some("[^0-9]".to_string()));
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }
}
