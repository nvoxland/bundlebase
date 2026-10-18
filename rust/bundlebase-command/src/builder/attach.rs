//! Attach command implementation.

use crate::parser::extract_string_content;
use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::AttachBlockOp;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to attach a data block to the bundle.
#[derive(Debug, Clone)]
pub struct AttachCommand {
    /// The path/URL of the data to attach
    pub path: String,
    /// The pack to attach to (None or "base" for base pack, otherwise join name)
    pub pack: Option<String>,
    /// When true, suppress the auto-reindex hook so existing indexes remain
    /// stale until the user runs an explicit REINDEX.
    pub no_index: bool,
}

impl AttachCommand {
    /// Create a new AttachCommand.
    pub fn new(path: impl Into<String>, pack: Option<String>) -> Self {
        Self {
            path: path.into(),
            pack,
            no_index: false,
        }
    }

    /// Builder helper: set the `NO INDEX` flag.
    pub fn with_no_index(mut self, no_index: bool) -> Self {
        self.no_index = no_index;
        self
    }
}

impl CommandParsing for AttachCommand {
    fn rule() -> Rule {
        Rule::attach_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut pack = None;
        let mut no_index = false;
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
                Rule::no_index => {
                    no_index = true;
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

        let path =
            path.ok_or_else(|| -> BundlebaseError { "ATTACH statement missing path".into() })?;

        Ok(AttachCommand::new(path, pack).with_no_index(no_index))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        let mut out = match &self.pack {
            Some(pack) if pack != "base" => {
                format!(
                    "ATTACH {} TO {}",
                    escape_string(&self.path),
                    quote_identifier(pack)
                )
            }
            _ => format!("ATTACH {}", escape_string(&self.path)),
        };
        if self.no_index {
            out.push_str(" NO INDEX");
        }
        out
    }
}

impl BundleBuilderCommand for AttachCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        if self.no_index {
            builder.suppress_auto_reindex_for_current_change();
        }
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;
        let pack_name = self.pack.as_deref().unwrap_or("base");

        let temp_reader = builder
            .bundle()
            .reader_factory
            .detect(&self.path, &bundlebase_data::BlockId::generate(), builder)
            .await?;
        let format = temp_reader.format();

        let op = AttachBlockOp::setup(
            &pack_id, &self.path, format, None, None, None, builder, None,
        )
        .await?;
        // Always-rules re-apply to the NEWLY attached base-pack block(s) only.
        // Re-running them against existing blocks would re-tombstone
        // already-deleted rows and, for non-idempotent SET expressions
        // (e.g. `SET n = n + 1`), corrupt their already-updated values on
        // every attach (bb-c6l). Capture the base-pack block indices the
        // attach adds; an attach to a join pack adds none, so the rules
        // correctly apply to nothing.
        let blocks_before = builder.base_pack_block_count();
        builder.apply_operation(op.into()).await?;
        let blocks_after = builder.base_pack_block_count();
        let new_block_indices: Vec<usize> = (blocks_before..blocks_after).collect();

        // Apply always-delete rules to the newly attached data
        let rules = builder.always_delete_rules();
        if !rules.is_empty() && !new_block_indices.is_empty() {
            for rule in &rules {
                let delete_rowids = builder
                    .select_row_ids_in_blocks(rule, &new_block_indices)
                    .await?;
                if !delete_rowids.is_empty() {
                    tracing::debug!(
                        "[ALWAYS DELETE] Auto-deleted {} rows matching WHERE {}",
                        delete_rowids.len(),
                        rule
                    );
                    // Tombstones are the sole representation; mark_deleted
                    // mirrors them into the blocks for in-session visibility.
                    builder.mark_deleted(delete_rowids, rule);
                }
            }
        }

        // Apply always-update rules to the newly attached data
        let update_rules = builder.always_update_rules();
        if !update_rules.is_empty() && !new_block_indices.is_empty() {
            for rule in &update_rules {
                let full_sql = format!(
                    "UPDATE bundle SET {} WHERE {}",
                    rule.set_clause, rule.where_clause
                );
                let cmd = crate::parser::parse_command(&full_sql)?;
                if let crate::BundleCommand::Update(update_cmd) = cmd {
                    let columns: Vec<String> = update_cmd
                        .assignments
                        .iter()
                        .map(|a| a.column.clone())
                        .collect();
                    let expressions: Vec<String> = update_cmd
                        .assignments
                        .iter()
                        .map(|a| a.expression.clone())
                        .collect();

                    let updated = builder
                        .evaluate_update_cols_in_blocks(
                            &columns,
                            &expressions,
                            &rule.where_clause,
                            &new_block_indices,
                        )
                        .await?;
                    if updated > 0 {
                        tracing::debug!(
                            "[ALWAYS UPDATE] Auto-updated {} rows matching SET {} WHERE {}",
                            updated,
                            rule.set_clause,
                            rule.where_clause
                        );
                        // The overlay is the sole representation — flushed
                        // to the blocks for in-session visibility.
                        builder.flush_pending_updates_to_blocks()?;
                    }
                }
            }
        }

        Ok(format!("Attached {} to {}", self.path, pack_name))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test setup and assertions may panic on failure.
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

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
    fn test_parse_attach_no_index() {
        let cmd = parse_command("ATTACH 'data.parquet' NO INDEX").unwrap();
        match cmd {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "data.parquet");
                assert!(c.no_index, "NO INDEX flag should be set");
            }
            _ => panic!("Expected Attach variant"),
        }

        let cmd = parse_command("ATTACH 'd.csv' TO users NO INDEX").unwrap();
        match cmd {
            BundleCommand::Attach(c) => {
                assert_eq!(c.pack, Some("users".to_string()));
                assert!(c.no_index);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_attach_no_index_round_trip() {
        let cmd = AttachCommand::new("data.csv", None).with_no_index(true);
        let statement = cmd.to_statement();
        assert_eq!(statement, "ATTACH 'data.csv' NO INDEX");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Attach(c) => assert!(c.no_index),
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
