//! Update command implementation.
//!
//! Evaluates SET expressions against matching rows, collects updated values
//! keyed by RowId, and stores them in the builder's pending_updates accumulator.

use crate::{CommandParsing, Rule};
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use bundlebase::bundle::column_metadata;
use bundlebase::bundle::BundleFacade;
use tracing::debug;

/// A single SET assignment: column = expression
#[derive(Debug, Clone)]
pub struct SetAssignment {
    pub column: String,
    pub expression: String,
}

/// Command to update rows matching a WHERE condition.
#[derive(Debug, Clone)]
pub struct UpdateCommand {
    pub assignments: Vec<SetAssignment>,
    pub where_clause: String,
}

impl UpdateCommand {
    pub fn new(assignments: Vec<SetAssignment>, where_clause: impl Into<String>) -> Self {
        Self {
            assignments,
            where_clause: where_clause.into(),
        }
    }
}

impl CommandParsing for UpdateCommand {
    fn rule() -> Rule {
        Rule::update_stmt
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
            "UPDATE statement missing WHERE clause".into()
        })?;

        if assignments.is_empty() {
            return Err("UPDATE statement missing SET assignments".into());
        }

        Ok(UpdateCommand::new(assignments, where_clause))
    }

    fn to_statement(&self) -> String {
        let sets: Vec<String> = self.assignments.iter()
            .map(|a| format!("{} = {}", a.column, a.expression))
            .collect();
        format!("UPDATE bundle SET {} WHERE {}", sets.join(", "), self.where_clause)
    }
}

impl BundleBuilderCommand for UpdateCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Translate user-visible column names to stable col_<id> references
        let col_names = builder.column_names();
        let where_clause = column_metadata::translate_sql_to_col_ids(&self.where_clause, &col_names);

        // Translate column names and expressions to col_<id>
        let columns: Vec<String> = self.assignments.iter().map(|a| {
            column_metadata::translate_sql_to_col_ids(&a.column, &col_names)
        }).collect();
        let expressions: Vec<String> = self.assignments.iter().map(|a| {
            column_metadata::translate_sql_to_col_ids(&a.expression, &col_names)
        }).collect();

        let updated_count = builder.evaluate_update_cols(&columns, &expressions, &where_clause).await?;
        debug!("[UPDATE] Updated {} rows", updated_count);

        if updated_count > 0 {
            // Push updates to DataBlocks for scan-level visibility (direct queries)
            builder.flush_pending_updates_to_blocks();

            // Also apply a DataFrame-level SQL transform so FilterOp and other
            // DataFrame-level operations see the updated values.
            // Uses CASE WHEN to replace values matching the WHERE condition.
            // Build using col_<id> names from the internal column map.
            let assignment_map: std::collections::HashMap<&str, &str> = columns.iter()
                .zip(expressions.iter())
                .map(|(c, e)| (c.as_str(), e.as_str()))
                .collect();
            let select_cols: Vec<String> = col_names.keys().map(|col_id| {
                let col_name = column_metadata::col_id_name(col_id);
                let quoted = format!("\"{}\"", col_name);
                if let Some(expr) = assignment_map.get(col_name.as_str()) {
                    format!("CASE WHEN ({}) THEN ({}) ELSE {} END AS {}", where_clause, expr, quoted, quoted)
                } else {
                    quoted
                }
            }).collect();
            let filter_query = format!("SELECT {} FROM bundle", select_cols.join(", "));
            debug!("[UPDATE] Applying CASE WHEN transform for in-session visibility");
            builder.apply_operation(bundlebase::bundle::operation::FilterOp::new(&filter_query, vec![]).into()).await?;
        }

        Ok(format!("Updated {} rows", updated_count))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_update_simple() {
        let input = "UPDATE bundle SET salary = 100 WHERE id = 1";
        let cmd = parse_command(input).expect("Failed to parse UPDATE");
        match cmd {
            BundleCommand::Update(c) => {
                assert_eq!(c.assignments.len(), 1);
                assert_eq!(c.assignments[0].column, "salary");
                assert_eq!(c.assignments[0].expression, "100");
                assert_eq!(c.where_clause, "id = 1");
            }
            _ => panic!("Expected Update variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_update_expression() {
        let input = "UPDATE bundle SET salary = salary * 1.1 WHERE department = 'eng'";
        let cmd = parse_command(input).expect("Failed to parse UPDATE with expression");
        match cmd {
            BundleCommand::Update(c) => {
                assert_eq!(c.assignments[0].column, "salary");
                assert_eq!(c.assignments[0].expression, "salary * 1.1");
                assert_eq!(c.where_clause, "department = 'eng'");
            }
            _ => panic!("Expected Update variant"),
        }
    }

    #[test]
    fn test_parse_update_multiple_columns() {
        let input = "UPDATE bundle SET name = 'unknown', age = 0 WHERE name IS NULL";
        let cmd = parse_command(input).expect("Failed to parse multi-column UPDATE");
        match cmd {
            BundleCommand::Update(c) => {
                assert_eq!(c.assignments.len(), 2);
                assert_eq!(c.assignments[0].column, "name");
                assert_eq!(c.assignments[0].expression, "'unknown'");
                assert_eq!(c.assignments[1].column, "age");
                assert_eq!(c.assignments[1].expression, "0");
                assert_eq!(c.where_clause, "name IS NULL");
            }
            _ => panic!("Expected Update variant"),
        }
    }

    #[test]
    fn test_parse_update_null() {
        let input = "UPDATE bundle SET status = NULL WHERE inactive = true";
        let cmd = parse_command(input).expect("Failed to parse NULL UPDATE");
        match cmd {
            BundleCommand::Update(c) => {
                assert_eq!(c.assignments[0].expression, "NULL");
            }
            _ => panic!("Expected Update variant"),
        }
    }

    #[test]
    fn test_parse_update_case_insensitive() {
        let input = "update bundle set salary = 100 where id = 1";
        let cmd = parse_command(input).expect("Failed to parse lowercase UPDATE");
        match cmd {
            BundleCommand::Update(c) => {
                assert_eq!(c.assignments[0].column, "salary");
            }
            _ => panic!("Expected Update variant"),
        }
    }

    #[test]
    fn test_parse_update_roundtrip() {
        let cmd = UpdateCommand::new(
            vec![SetAssignment { column: "salary".to_string(), expression: "100".to_string() }],
            "id = 1",
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "UPDATE bundle SET salary = 100 WHERE id = 1");
    }
}
