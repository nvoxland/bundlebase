//! Always-update command implementation.
//!
//! Registers a persistent update rule AND immediately updates matching rows.

use crate::builder::update::SetAssignment;
use crate::{CommandParsing, Rule};
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use bundlebase::bundle::BundleFacade;
use bundlebase::bundle::operation::{AlwaysUpdateOp, FilterOp};
use tracing::debug;

/// Command to register an always-update rule.
#[derive(Debug, Clone)]
pub struct AlwaysUpdateCommand {
    pub assignments: Vec<SetAssignment>,
    pub where_clause: String,
}

impl AlwaysUpdateCommand {
    pub fn new(assignments: Vec<SetAssignment>, where_clause: impl Into<String>) -> Self {
        Self {
            assignments,
            where_clause: where_clause.into(),
        }
    }

    /// Returns the SET clause as a string for storage.
    pub fn set_clause_text(&self) -> String {
        self.assignments.iter()
            .map(|a| format!("{} = {}", a.column, a.expression))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl CommandParsing for AlwaysUpdateCommand {
    fn rule() -> Rule {
        Rule::always_update_stmt
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

        let where_clause = where_clause.ok_or_else(|| -> BundlebaseError {
            "ALWAYS UPDATE statement missing WHERE clause".into()
        })?;

        if assignments.is_empty() {
            return Err("ALWAYS UPDATE statement missing SET assignments".into());
        }

        Ok(AlwaysUpdateCommand::new(assignments, where_clause))
    }

    fn to_statement(&self) -> String {
        format!("ALWAYS UPDATE bundle SET {} WHERE {}", self.set_clause_text(), self.where_clause)
    }
}

impl BundleBuilderCommand for AlwaysUpdateCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let columns: Vec<String> = self.assignments.iter().map(|a| a.column.clone()).collect();
        let expressions: Vec<String> = self.assignments.iter().map(|a| a.expression.clone()).collect();
        let set_clause = self.set_clause_text();

        // 1. Immediately update matching rows (same as regular UPDATE)
        let updated_count = builder.evaluate_update_cols(&columns, &expressions, &self.where_clause).await?;
        debug!("[ALWAYS UPDATE] Updated {} existing rows", updated_count);

        if updated_count > 0 {
            builder.flush_pending_updates_to_blocks();

            let schema = builder.schema().await?;
            let select_cols: Vec<String> = schema.fields().iter().map(|f| {
                let name = f.name();
                let quoted = format!("\"{}\"", name);
                if let Some(assignment) = self.assignments.iter().find(|a| a.column == *name) {
                    format!("CASE WHEN ({}) THEN ({}) ELSE {} END AS {}", self.where_clause, assignment.expression, quoted, quoted)
                } else {
                    quoted
                }
            }).collect();
            let filter_query = format!("SELECT {} FROM bundle", select_cols.join(", "));
            builder.apply_operation(FilterOp::new(&filter_query, vec![]).into()).await?;
        }

        // 2. Register the persistent always-update rule
        builder.apply_operation(AlwaysUpdateOp::new(&set_clause, &self.where_clause).into()).await?;

        Ok(format!("Always-update rule added (updated {} existing rows)", updated_count))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_always_update() {
        let input = "ALWAYS UPDATE bundle SET salary = 0 WHERE salary < 0";
        let cmd = parse_command(input).expect("Failed to parse ALWAYS UPDATE");
        match cmd {
            BundleCommand::AlwaysUpdate(c) => {
                assert_eq!(c.assignments.len(), 1);
                assert_eq!(c.assignments[0].column, "salary");
                assert_eq!(c.assignments[0].expression, "0");
                assert_eq!(c.where_clause, "salary < 0");
            }
            _ => panic!("Expected AlwaysUpdate variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_always_update_multiple_columns() {
        let input = "ALWAYS UPDATE bundle SET name = 'unknown', age = 0 WHERE name IS NULL";
        let cmd = parse_command(input).expect("Failed to parse multi-column ALWAYS UPDATE");
        match cmd {
            BundleCommand::AlwaysUpdate(c) => {
                assert_eq!(c.assignments.len(), 2);
                assert_eq!(c.assignments[0].column, "name");
                assert_eq!(c.assignments[0].expression, "'unknown'");
                assert_eq!(c.assignments[1].column, "age");
                assert_eq!(c.assignments[1].expression, "0");
                assert_eq!(c.where_clause, "name IS NULL");
            }
            _ => panic!("Expected AlwaysUpdate variant"),
        }
    }

    #[test]
    fn test_parse_always_update_case_insensitive() {
        let input = "always update bundle set salary = 100 where id = 1";
        let cmd = parse_command(input).expect("Failed to parse lowercase");
        match cmd {
            BundleCommand::AlwaysUpdate(c) => {
                assert_eq!(c.assignments[0].column, "salary");
                assert_eq!(c.assignments[0].expression, "100");
                assert_eq!(c.where_clause, "id = 1");
            }
            _ => panic!("Expected AlwaysUpdate variant"),
        }
    }

    #[test]
    fn test_parse_always_update_roundtrip() {
        let cmd = AlwaysUpdateCommand::new(
            vec![SetAssignment { column: "salary".to_string(), expression: "0".to_string() }],
            "salary < 0",
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "ALWAYS UPDATE bundle SET salary = 0 WHERE salary < 0");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::AlwaysUpdate(c) => {
                assert_eq!(c.assignments[0].column, "salary");
                assert_eq!(c.where_clause, "salary < 0");
            }
            _ => panic!("Expected AlwaysUpdate variant"),
        }
    }
}
