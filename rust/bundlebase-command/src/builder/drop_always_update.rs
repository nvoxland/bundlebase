//! Drop always-update command implementation.

use crate::builder::update::SetAssignment;
use crate::{CommandParsing, Rule};
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use bundlebase::bundle::operation::DropAlwaysUpdateOp;

/// Command to remove always-update rules.
#[derive(Debug, Clone)]
pub struct DropAlwaysUpdateCommand {
    /// None = drop all rules, Some = drop specific rule by "SET ... WHERE ..." text
    pub rule_text: Option<String>,
}

impl DropAlwaysUpdateCommand {
    pub fn new(rule_text: Option<String>) -> Self {
        Self { rule_text }
    }
}

impl CommandParsing for DropAlwaysUpdateCommand {
    fn rule() -> Rule {
        Rule::drop_always_update_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut assignments = Vec::new();
        let mut where_clause = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::update_set_clause => {
                    for assignment_pair in inner_pair.into_inner() {
                        if let Rule::update_assignment = assignment_pair.as_rule() {
                            let mut col = None;
                            let mut expr = None;
                            for part in assignment_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        col = Some(part.as_str().trim().to_string());
                                    }
                                    Rule::update_value_expr => {
                                        expr = Some(part.as_str().trim().to_string());
                                    }
                                    _ => {}
                                }
                            }
                            if let (Some(column), Some(expression)) = (col, expr) {
                                assignments.push(SetAssignment { column, expression });
                            }
                        }
                    }
                }
                Rule::update_where_clause => {
                    where_clause = Some(inner_pair.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        let rule_text = if !assignments.is_empty() {
            let set_clause: String = assignments.iter()
                .map(|a| format!("{} = {}", a.column, a.expression))
                .collect::<Vec<_>>()
                .join(", ");
            let wc = where_clause.ok_or_else(|| -> BundlebaseError {
                "DROP ALWAYS UPDATE with SET clause requires WHERE clause".into()
            })?;
            Some(format!("SET {} WHERE {}", set_clause, wc))
        } else {
            None
        };

        Ok(DropAlwaysUpdateCommand::new(rule_text))
    }

    fn to_statement(&self) -> String {
        match &self.rule_text {
            Some(rt) => format!("DROP ALWAYS UPDATE {}", rt),
            None => "DROP ALWAYS UPDATE".to_string(),
        }
    }
}

impl BundleBuilderCommand for DropAlwaysUpdateCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropAlwaysUpdateOp::new(self.rule_text.clone());
        builder.apply_operation(op.into()).await?;

        match &self.rule_text {
            Some(rt) => Ok(format!("Dropped always-update rule: {}", rt)),
            None => Ok("Dropped all always-update rules".to_string()),
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
        let input = "DROP ALWAYS UPDATE";
        let cmd = parse_command(input).expect("Failed to parse DROP ALWAYS UPDATE");
        match cmd {
            BundleCommand::DropAlwaysUpdate(c) => {
                assert!(c.rule_text.is_none());
            }
            _ => panic!("Expected DropAlwaysUpdate variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_drop_specific() {
        let input = "DROP ALWAYS UPDATE SET salary = 0 WHERE salary < 0";
        let cmd = parse_command(input).expect("Failed to parse");
        match cmd {
            BundleCommand::DropAlwaysUpdate(c) => {
                assert_eq!(c.rule_text, Some("SET salary = 0 WHERE salary < 0".to_string()));
            }
            _ => panic!("Expected DropAlwaysUpdate variant"),
        }
    }

    #[test]
    fn test_parse_drop_roundtrip() {
        let cmd = DropAlwaysUpdateCommand::new(Some("SET x = 1 WHERE x > 5".to_string()));
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP ALWAYS UPDATE SET x = 1 WHERE x > 5");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::DropAlwaysUpdate(c) => {
                assert_eq!(c.rule_text, Some("SET x = 1 WHERE x > 5".to_string()));
            }
            _ => panic!("Expected DropAlwaysUpdate variant"),
        }
    }
}
