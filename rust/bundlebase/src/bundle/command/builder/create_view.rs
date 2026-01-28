//! CreateView command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::CreateViewOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to create a view from a SQL statement.
#[derive(Debug, Clone)]
pub struct CreateViewCommand {
    /// The name of the view
    pub name: String,
    /// The SQL query that defines the view
    pub sql: String,
}

impl CreateViewCommand {
    /// Create a new CreateViewCommand.
    pub fn new(name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sql: sql.into(),
        }
    }
}

impl CommandParsing for CreateViewCommand {
    fn rule() -> Rule {
        Rule::create_view_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut sql = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    name = Some(inner.as_str().to_string());
                }
                Rule::view_sql => {
                    sql = Some(inner.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError { "CREATE VIEW missing name".into() })?;
        let sql = sql.ok_or_else(|| -> BundlebaseError { "CREATE VIEW missing SQL".into() })?;

        Ok(CreateViewCommand::new(name, sql))
    }

    fn to_statement(&self) -> String {
        format!("CREATE VIEW {} AS {}", self.name, self.sql)
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateViewCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let (op, _view_builder) = CreateViewOp::setup(&self.name, &self.sql, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Created view: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::{BundleCommand, CommandParsing};
    use crate::bundle::command::parser::{BundlebaseParser, Rule};
    use pest::Parser;

    fn parse_create_view(input: &str) -> Result<CreateViewCommand, crate::BundlebaseError> {
        let mut pairs = BundlebaseParser::parse(Rule::statement, input)
            .map_err(|e| crate::BundlebaseError::from(e.to_string()))?;

        let statement = pairs.next().unwrap();
        let category_stmt = statement.into_inner().next().unwrap();
        let inner_stmt = category_stmt.into_inner().next().unwrap();

        CreateViewCommand::from_statement(inner_stmt)
    }

    #[test]
    fn test_parse_create_view() {
        let input = "CREATE VIEW adults AS SELECT * FROM bundle WHERE age > 21";
        let cmd = parse_create_view(input).unwrap();
        assert_eq!(cmd.name, "adults");
        assert_eq!(cmd.sql, "SELECT * FROM bundle WHERE age > 21");
    }

    #[test]
    fn test_parse_create_view_complex_sql() {
        let input = "CREATE VIEW high_earners AS SELECT id, name, salary FROM bundle WHERE salary > 100000 ORDER BY salary DESC";
        let cmd = parse_create_view(input).unwrap();
        assert_eq!(cmd.name, "high_earners");
        assert_eq!(cmd.sql, "SELECT id, name, salary FROM bundle WHERE salary > 100000 ORDER BY salary DESC");
    }

    #[test]
    fn test_round_trip() {
        let cmd = CreateViewCommand::new("my_view", "SELECT * FROM bundle WHERE x > 5");
        let statement = cmd.to_statement();
        assert_eq!(statement, "CREATE VIEW my_view AS SELECT * FROM bundle WHERE x > 5");

        let cmd2 = parse_create_view(&statement).unwrap();
        assert_eq!(cmd2.name, "my_view");
        assert_eq!(cmd2.sql, "SELECT * FROM bundle WHERE x > 5");
    }

    #[test]
    fn test_parse_command_succeeds() {
        let input = "CREATE VIEW adults AS SELECT * FROM bundle WHERE age > 21";
        let result = parse_command(input);
        assert!(result.is_ok(), "CREATE VIEW should parse successfully: {:?}", result.err());
        let cmd = result.unwrap();
        match cmd {
            BundleCommand::CreateView(create_view) => {
                assert_eq!(create_view.name, "adults");
                assert_eq!(create_view.sql, "SELECT * FROM bundle WHERE age > 21");
            }
            _ => panic!("Expected CreateView command"),
        }
    }
}
