//! Attach command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{CommandParsing, Rule};
use crate::parser::extract_string_content;
use bundlebase::bundle::operation::AttachBlockOp;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use bundlebase::bundle::BundleFacade;

/// Command to attach a data block to the bundle.
#[derive(Debug, Clone)]
pub struct AttachCommand {
    /// The path/URL of the data to attach
    pub path: String,
    /// The pack to attach to (None or "base" for base pack, otherwise join name)
    pub pack: Option<String>,
}

impl AttachCommand {
    /// Create a new AttachCommand.
    pub fn new(path: impl Into<String>, pack: Option<String>) -> Self {
        Self {
            path: path.into(),
            pack,
        }
    }
}

impl CommandParsing for AttachCommand {
    fn rule() -> Rule {
        Rule::attach_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut pack = None;
        let raw = pair.as_str().to_string();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::quoted_string => {
                    if path.is_none() {
                        path = Some(extract_string_content(inner_pair.as_str())?);
                    }
                }
                Rule::identifier => {
                    // The identifier after TO is the pack name
                    if pack.is_none() {
                        pack = Some(extract_identifier(&inner_pair));
                    }
                }
                _ => {}
            }
        }

        // If pack wasn't captured from inner pairs, try to extract from raw string
        if pack.is_none() {
            let upper = raw.to_uppercase();
            if let Some(to_pos) = upper.find(" TO ") {
                let after_to = raw[to_pos + 4..].trim_start();
                let pack_name: String = after_to
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !pack_name.is_empty() {
                    pack = Some(pack_name);
                }
            }
        }

        let path = path.ok_or_else(|| -> BundlebaseError {
            "ATTACH statement missing path".into()
        })?;

        Ok(AttachCommand::new(path, pack))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        match &self.pack {
            Some(pack) if pack != "base" => {
                format!("ATTACH {} TO {}", escape_string(&self.path), quote_identifier(pack))
            }
            _ => format!("ATTACH {}", escape_string(&self.path)),
        }
    }
}

impl BundleBuilderCommand for AttachCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;
        let pack_name = self.pack.as_deref().unwrap_or("base");

        let temp_reader = builder.bundle().reader_factory
            .detect(&self.path, &bundlebase_data::BlockId::generate(), builder)
            .await?;
        let format = temp_reader.format();

        let op = AttachBlockOp::setup(&pack_id, &self.path, format, None, None, None, builder).await?;
        builder.apply_operation(op.into()).await?;

        // Apply always-delete rules to the newly attached data
        let rules = builder.always_delete_rules();
        if !rules.is_empty() {
            for rule in &rules {
                let delete_rowids = builder.select_row_ids(rule).await?;
                if !delete_rowids.is_empty() {
                    tracing::debug!(
                        "[ALWAYS DELETE] Auto-deleted {} rows matching WHERE {}",
                        delete_rowids.len(),
                        rule
                    );
                    builder.mark_deleted(delete_rowids, rule);

                    let filter_query = format!(
                        "SELECT * FROM bundle WHERE NOT ({})",
                        rule
                    );
                    builder.apply_operation(
                        bundlebase::bundle::operation::FilterOp::new(&filter_query, vec![]).into()
                    ).await?;
                }
            }
        }

        // Apply always-update rules to the newly attached data
        let update_rules = builder.always_update_rules();
        if !update_rules.is_empty() {
            for rule in &update_rules {
                let full_sql = format!("UPDATE bundle SET {} WHERE {}", rule.set_clause, rule.where_clause);
                let cmd = crate::parser::parse_command(&full_sql)?;
                if let crate::BundleCommand::Update(update_cmd) = cmd {
                    let columns: Vec<String> = update_cmd.assignments.iter().map(|a| a.column.clone()).collect();
                    let expressions: Vec<String> = update_cmd.assignments.iter().map(|a| a.expression.clone()).collect();

                    let updated = builder.evaluate_update_cols(&columns, &expressions, &rule.where_clause).await?;
                    if updated > 0 {
                        tracing::debug!(
                            "[ALWAYS UPDATE] Auto-updated {} rows matching SET {} WHERE {}",
                            updated,
                            rule.set_clause,
                            rule.where_clause
                        );
                        builder.flush_pending_updates_to_blocks();

                        // Build CASE WHEN using internal name column names from the internal schema
                        let col_names_map = builder.bundle_schema();
                        let select_cols: Vec<String> = col_names_map.keys().map(|col_id| {
                            let internal_name = col_names_map.internal_name(col_id).expect("column ID from schema keys");
                            let quoted = format!("\"{}\"", internal_name);
                            if let Some(assignment) = update_cmd.assignments.iter().find(|a| a.column == internal_name) {
                                format!("CASE WHEN ({}) THEN ({}) ELSE {} END AS {}", rule.where_clause, assignment.expression, quoted, quoted)
                            } else {
                                quoted
                            }
                        }).collect();
                        let filter_query = format!("SELECT {} FROM bundle", select_cols.join(", "));
                        builder.apply_operation(
                            bundlebase::bundle::operation::FilterOp::new(&filter_query, vec![]).into()
                        ).await?;
                    }
                }
            }
        }

        Ok(format!("Attached {} to {}", self.path, pack_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_attach_simple() {
        let input = "ATTACH 'data.parquet'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "data.parquet");
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_with_pack() {
        let input = "ATTACH 'more_users.parquet' TO users";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "more_users.parquet");
                assert_eq!(c.pack, Some("users".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = AttachCommand::new("data.csv", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "ATTACH 'data.csv'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "data.csv");
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_round_trip_with_pack() {
        let cmd = AttachCommand::new("orders.parquet", Some("orders".to_string()));
        let statement = cmd.to_statement();
        assert_eq!(statement, "ATTACH 'orders.parquet' TO orders");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "orders.parquet");
                assert_eq!(c.pack, Some("orders".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }
}
