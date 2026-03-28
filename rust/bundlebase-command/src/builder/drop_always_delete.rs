//! Drop always-delete command implementation.

use crate::{CommandParsing, Rule};
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use bundlebase::bundle::operation::DropAlwaysDeleteOp;

/// Command to remove always-delete rules.
#[derive(Debug, Clone)]
pub struct DropAlwaysDeleteCommand {
    /// None = drop all rules, Some = drop specific rule
    pub where_clause: Option<String>,
}

impl DropAlwaysDeleteCommand {
    pub fn new(where_clause: Option<String>) -> Self {
        Self { where_clause }
    }
}

impl CommandParsing for DropAlwaysDeleteCommand {
    fn rule() -> Rule {
        Rule::drop_always_delete_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut where_clause = None;

        for inner_pair in pair.into_inner() {
            if let Rule::delete_where_clause = inner_pair.as_rule() {
                where_clause = Some(inner_pair.as_str().trim().to_string());
            }
        }

        Ok(DropAlwaysDeleteCommand::new(where_clause))
    }

    fn to_statement(&self) -> String {
        match &self.where_clause {
            Some(wc) => format!("DROP ALWAYS DELETE WHERE {}", wc),
            None => "DROP ALWAYS DELETE".to_string(),
        }
    }
}

impl BundleBuilderCommand for DropAlwaysDeleteCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropAlwaysDeleteOp::new(self.where_clause.clone());
        builder.apply_operation(op.into()).await?;

        match &self.where_clause {
            Some(wc) => Ok(format!("Dropped always-delete rule: WHERE {}", wc)),
            None => Ok("Dropped all always-delete rules".to_string()),
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_drop_all() {
        let input = "DROP ALWAYS DELETE";
        let cmd = parse_command(input).expect("Failed to parse DROP ALWAYS DELETE");
        match cmd {
            BundleCommand::DropAlwaysDelete(c) => {
                assert!(c.where_clause.is_none());
            }
            _ => panic!("Expected DropAlwaysDelete variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_drop_specific() {
        let input = "DROP ALWAYS DELETE WHERE salary < 0";
        let cmd = parse_command(input).expect("Failed to parse");
        match cmd {
            BundleCommand::DropAlwaysDelete(c) => {
                assert_eq!(c.where_clause, Some("salary < 0".to_string()));
            }
            _ => panic!("Expected DropAlwaysDelete variant"),
        }
    }

    #[test]
    fn test_parse_drop_roundtrip() {
        let cmd = DropAlwaysDeleteCommand::new(Some("x > 5".to_string()));
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::DropAlwaysDelete(c) => {
                assert_eq!(c.where_clause, Some("x > 5".to_string()));
            }
            _ => panic!("Expected DropAlwaysDelete variant"),
        }
    }
}
