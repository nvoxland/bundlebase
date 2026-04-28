//! AddColumn command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::AddColumnOp;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to add a computed column to a bundle.
#[derive(Debug, Clone)]
pub struct AddColumnCommand {
    /// The name for the new column
    pub name: String,
    /// The SQL expression to compute the column value
    pub expression: String,
}

impl AddColumnCommand {
    /// Create a new AddColumnCommand.
    pub fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
        }
    }
}

impl CommandParsing for AddColumnCommand {
    fn rule() -> Rule {
        Rule::add_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut expression = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    name = Some(extract_identifier(&inner));
                }
                Rule::add_column_expression => {
                    expression = Some(inner.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "ADD COLUMN statement missing column name".into()
        })?;
        let expression = expression.ok_or_else(|| -> BundlebaseError {
            "ADD COLUMN statement missing expression".into()
        })?;

        Ok(AddColumnCommand::new(name, expression))
    }

    fn to_statement(&self) -> String {
        format!(
            "ADD COLUMN {} AS {}",
            quote_identifier(&self.name),
            self.expression
        )
    }
}

impl BundleBuilderCommand for AddColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Translate column names in the expression to stable internal name references
        let translated_expression = builder.translate_sql(&self.expression);

        builder
            .apply_operation(AddColumnOp::setup(&self.name, &translated_expression).into())
            .await?;

        Ok(format!("Added column {} AS {}", self.name, self.expression))
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_command;
    use crate::{BundleCommand, CommandParsing};

    #[test]
    fn test_parse_add_column() {
        let cmd = parse_command("ADD COLUMN full_name AS first_name || ' ' || last_name").unwrap();
        match cmd {
            BundleCommand::AddColumn(c) => {
                assert_eq!(c.name, "full_name");
                assert_eq!(c.expression, "first_name || ' ' || last_name");
            }
            other => panic!("Expected AddColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_add_column_case_insensitive() {
        let cmd = parse_command("add column total as price * quantity").unwrap();
        match cmd {
            BundleCommand::AddColumn(c) => {
                assert_eq!(c.name, "total");
                assert_eq!(c.expression, "price * quantity");
            }
            other => panic!("Expected AddColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = super::AddColumnCommand::new("full_name", "first_name || ' ' || last_name");
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "ADD COLUMN full_name AS first_name || ' ' || last_name"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::AddColumn(c) => {
                assert_eq!(c.name, "full_name");
                assert_eq!(c.expression, "first_name || ' ' || last_name");
            }
            other => panic!("Expected AddColumn, got {:?}", other),
        }
    }
}
