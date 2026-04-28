//! DropIndex command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::DropIndexOp;
use bundlebase::BundleBuilder;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;

/// Command to drop an index by name or column.
#[derive(Debug, Clone)]
pub struct DropIndexCommand {
    /// The index name or column name to identify the index to drop
    pub identifier: String,
}

impl DropIndexCommand {
    /// Create a new DropIndexCommand.
    pub fn new(identifier: impl Into<String>) -> Self {
        Self {
            identifier: identifier.into(),
        }
    }
}

impl CommandParsing for DropIndexCommand {
    fn rule() -> Rule {
        Rule::drop_index_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut identifier = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::identifier {
                identifier = Some(extract_identifier(&inner_pair));
            }
        }

        let identifier = identifier.ok_or_else(|| -> BundlebaseError {
            "DROP INDEX statement missing index name or column name".into()
        })?;

        Ok(DropIndexCommand::new(identifier))
    }

    fn to_statement(&self) -> String {
        format!("DROP INDEX {}", quote_identifier(&self.identifier))
    }
}

impl BundleBuilderCommand for DropIndexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Find the index ID: look up by name first, then fall back to column name match
        let index_id = {
            let indexes = builder.indexes();

            // First try matching by index name
            let index = indexes.iter().find(|idx| idx.name() == self.identifier);

            // Fall back to matching by column name -> column ID -> index
            let index = index.or_else(|| {
                let col_names = builder.bundle_schema();
                // Find the ColumnId for the given column name
                let target_col_id = col_names
                    .iter()
                    .find(|(_, name)| name.as_str() == self.identifier)
                    .map(|(id, _)| *id);

                if let Some(col_id) = target_col_id {
                    indexes
                        .iter()
                        .find(|idx| idx.column_ids().contains(&col_id))
                } else {
                    None
                }
            });

            match index {
                Some(idx) => *idx.id(),
                None => {
                    return Err(format!("No index found matching '{}'", self.identifier).into());
                }
            }
        };

        builder
            .apply_operation(DropIndexOp::setup(&index_id).await?.into())
            .await?;

        Ok(format!("Dropped index: {}", self.identifier))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_drop_index() {
        let input = "DROP INDEX user_id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropIndex(c) => {
                assert_eq!(c.identifier, "user_id");
            }
            _ => panic!("Expected DropIndex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropIndexCommand::new("email");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP INDEX email");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropIndex(c) => {
                assert_eq!(c.identifier, "email");
            }
            _ => panic!("Expected DropIndex variant"),
        }
    }
}
