//! RebuildIndex command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::BundleBuilder;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;

/// Command to rebuild an index on a column.
#[derive(Debug, Clone)]
pub struct RebuildIndexCommand {
    /// The column name to rebuild the index for
    pub column: String,
}

impl RebuildIndexCommand {
    /// Create a new RebuildIndexCommand.
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
        }
    }
}

impl CommandParsing for RebuildIndexCommand {
    fn rule() -> Rule {
        Rule::rebuild_index_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut column = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                column = Some(extract_identifier(&inner));
            }
        }

        let column = column.ok_or_else(|| -> BundlebaseError {
            "REBUILD INDEX statement missing column name".into()
        })?;

        Ok(RebuildIndexCommand::new(column))
    }

    fn to_statement(&self) -> String {
        format!("REBUILD INDEX ON {}", quote_identifier(&self.column))
    }
}

impl BundleBuilderCommand for RebuildIndexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // REBUILD implies an index exists — silently succeeding when none does
        // misleads callers into believing their queries are indexed (bb-cld:
        // benchmarks used rebuild_index as if it created indexes and measured
        // plain scans for months). Match by index name or column name, the
        // same resolution DROP INDEX uses.
        let exists = {
            let indexes = builder.indexes();
            let by_name = indexes.iter().any(|idx| idx.name() == self.column);
            by_name || {
                let col_names = builder.bundle_schema();
                col_names
                    .iter()
                    .find(|(_, name)| name.as_str() == self.column)
                    .map(|(id, _)| *id)
                    .is_some_and(|col_id| {
                        indexes.iter().any(|idx| idx.column_ids().contains(&col_id))
                    })
            }
        };
        if !exists {
            return Err(format!(
                "No index exists on '{}' — create one first with CREATE INDEX",
                self.column
            )
            .into());
        }
        builder.reindex_internal().await?;
        Ok(format!("Rebuilt index on column: {}", self.column))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_rebuild_index() {
        let input = "REBUILD INDEX ON user_id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RebuildIndex(c) => {
                assert_eq!(c.column, "user_id");
            }
            _ => panic!("Expected RebuildIndex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RebuildIndexCommand::new("email");
        let statement = cmd.to_statement();
        assert_eq!(statement, "REBUILD INDEX ON email");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RebuildIndex(c) => {
                assert_eq!(c.column, "email");
            }
            _ => panic!("Expected RebuildIndex variant"),
        }
    }
}
