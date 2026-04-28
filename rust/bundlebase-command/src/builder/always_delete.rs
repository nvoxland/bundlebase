//! Always-delete command implementation.
//!
//! Registers a persistent delete rule AND immediately deletes matching rows.

use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::{AlwaysDeleteOp, FilterOp};
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;
use tracing::debug;

/// Command to register an always-delete rule.
#[derive(Debug, Clone)]
pub struct AlwaysDeleteCommand {
    pub where_clause: String,
}

impl AlwaysDeleteCommand {
    pub fn new(where_clause: impl Into<String>) -> Self {
        Self {
            where_clause: where_clause.into(),
        }
    }
}

impl CommandParsing for AlwaysDeleteCommand {
    fn rule() -> Rule {
        Rule::always_delete_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut where_clause = None;

        for inner_pair in pair.into_inner() {
            if let Rule::delete_where_clause = inner_pair.as_rule() {
                where_clause = Some(inner_pair.as_str().trim().to_string());
            }
        }

        let where_clause = where_clause.ok_or_else(|| -> BundlebaseError {
            "ALWAYS DELETE statement missing WHERE clause".into()
        })?;

        if where_clause.is_empty() {
            return Err("ALWAYS DELETE WHERE clause cannot be empty".into());
        }

        Ok(AlwaysDeleteCommand::new(where_clause))
    }

    fn to_statement(&self) -> String {
        format!("ALWAYS DELETE FROM bundle WHERE {}", self.where_clause)
    }
}

impl BundleBuilderCommand for AlwaysDeleteCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Translate user-visible column names to stable internal name references
        let where_clause = builder.translate_sql(&self.where_clause);

        // 1. Immediately delete matching rows (same as regular DELETE)
        let delete_rowids = builder.select_row_ids(&where_clause).await?;
        let deleted_count = delete_rowids.len();
        debug!(
            "[ALWAYS DELETE] Collected {} RowIds for WHERE {}",
            deleted_count, where_clause
        );

        if !delete_rowids.is_empty() {
            builder.mark_deleted(delete_rowids, &where_clause);

            let filter_query = format!("SELECT * FROM bundle WHERE NOT ({})", where_clause);
            builder
                .apply_operation(FilterOp::new(&filter_query, vec![]).into())
                .await?;
        }

        // 2. Register the persistent always-delete rule (stored with internal name names)
        builder
            .apply_operation(AlwaysDeleteOp::new(&where_clause).into())
            .await?;

        Ok(format!(
            "Always-delete rule added (deleted {} existing rows)",
            deleted_count
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_always_delete() {
        let input = "ALWAYS DELETE FROM bundle WHERE salary < 0";
        let cmd = parse_command(input).expect("Failed to parse ALWAYS DELETE");
        match cmd {
            BundleCommand::AlwaysDelete(c) => {
                assert_eq!(c.where_clause, "salary < 0");
            }
            _ => panic!("Expected AlwaysDelete variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_always_delete_case_insensitive() {
        let input = "always delete from bundle where id = 42";
        let cmd = parse_command(input).expect("Failed to parse lowercase");
        match cmd {
            BundleCommand::AlwaysDelete(c) => {
                assert_eq!(c.where_clause, "id = 42");
            }
            _ => panic!("Expected AlwaysDelete variant"),
        }
    }

    #[test]
    fn test_parse_always_delete_roundtrip() {
        let cmd = AlwaysDeleteCommand::new("salary < 0");
        let statement = cmd.to_statement();
        assert_eq!(statement, "ALWAYS DELETE FROM bundle WHERE salary < 0");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::AlwaysDelete(c) => {
                assert_eq!(c.where_clause, "salary < 0");
            }
            _ => panic!("Expected AlwaysDelete variant"),
        }
    }
}
