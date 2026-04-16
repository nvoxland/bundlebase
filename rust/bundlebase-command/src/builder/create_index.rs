//! CreateIndex command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::CreateIndexOp;
use bundlebase::bundle::{BundleBuilder, BundleFacade};
use bundlebase_common::BundlebaseError;
use bundlebase_index::IndexType;

/// Command to create an index on one or more columns.
#[derive(Debug, Clone)]
pub struct CreateIndexCommand {
    /// The columns to index
    pub columns: Vec<String>,
    /// The type of index to create
    pub index_type: IndexType,
    /// The index name (None means auto-generate)
    pub name: Option<String>,
}

impl CreateIndexCommand {
    /// Create a new CreateIndexCommand.
    pub fn new(columns: Vec<String>, index_type: IndexType, name: Option<String>) -> Self {
        Self {
            columns,
            index_type,
            name,
        }
    }
}

impl CommandParsing for CreateIndexCommand {
    fn rule() -> Rule {
        Rule::create_index_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut column = None;
        let mut index_type_str = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::index_type => {
                    index_type_str = Some(inner.as_str().to_lowercase());
                }
                Rule::identifier => {
                    column = Some(extract_identifier(&inner));
                }
                _ => {}
            }
        }

        let column = column.ok_or_else(|| -> BundlebaseError {
            "CREATE INDEX statement missing column name".into()
        })?;

        let index_type_str = index_type_str.ok_or_else(|| -> BundlebaseError {
            "CREATE INDEX statement missing index type (BTREE or TEXT)".into()
        })?;

        let index_type: IndexType =
            index_type_str
                .parse()
                .map_err(|e: bundlebase_index::ParseIndexTypeError| {
                    BundlebaseError::from(e.to_string())
                })?;

        Ok(CreateIndexCommand::new(vec![column], index_type, None))
    }

    // TODO: to_statement() and from_statement() are asymmetric for text indexes.
    // to_statement() outputs name, multi-column, and tokenizer info that from_statement()
    // cannot parse back. Text index statements cannot round-trip through SQL serialization.
    // If these statements are stored in the operation log, this needs to be fixed by extending
    // the parser grammar to support the full text index syntax.
    fn to_statement(&self) -> String {
        match &self.index_type {
            IndexType::BTree => {
                let quoted_cols: Vec<String> =
                    self.columns.iter().map(|c| quote_identifier(c)).collect();
                format!("CREATE BTREE INDEX ON {}", quoted_cols.join(", "))
            }
            IndexType::Inverted { tokenizer } => {
                let cols = self.columns.join(", ");
                if let Some(name) = self.name.as_deref() {
                    format!(
                        "CREATE TEXT INDEX '{}' ON [{}] (tokenizer: {:?})",
                        name, cols, tokenizer
                    )
                } else {
                    format!(
                        "CREATE TEXT INDEX ON [{}] (tokenizer: {:?})",
                        cols, tokenizer
                    )
                }
            }
        }
    }
}

impl BundleBuilderCommand for CreateIndexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let CreateIndexCommand {
            columns,
            index_type,
            name,
        } = *self;

        let cols_display = columns.join(", ");

        // Resolve the index name: use the explicit name if provided, otherwise auto-generate.
        let resolved_name = match name {
            Some(n) => n,
            None => format!("idx_{}", columns.join("_")),
        };

        let mut column_ids = Vec::with_capacity(columns.len());
        for real_name in &columns {
            let id = builder.column_id(real_name).ok_or_else(|| {
                BundlebaseError::from(format!("Column '{}' not found", real_name))
            })?;
            column_ids.push(id);
        }

        let op = CreateIndexOp::setup(column_ids, index_type, resolved_name).await?;

        builder.apply_operation(op.into()).await?;

        builder.reindex_internal().await?;

        Ok(format!("Created index on column(s): {}", cols_display))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_create_btree_index() {
        let input = "CREATE BTREE INDEX ON user_id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateIndex(c) => {
                assert_eq!(c.columns, vec!["user_id"]);
                assert_eq!(c.index_type, IndexType::BTree);
            }
            _ => panic!("Expected CreateIndex variant"),
        }
    }

    #[test]
    fn test_parse_create_text_index() {
        let input = "CREATE TEXT INDEX ON description";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateIndex(c) => {
                assert_eq!(c.columns, vec!["description"]);
                assert!(c.index_type.is_inverted());
            }
            _ => panic!("Expected CreateIndex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = CreateIndexCommand::new(vec!["email".to_string()], IndexType::BTree, None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "CREATE BTREE INDEX ON email");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateIndex(c) => {
                assert_eq!(c.columns, vec!["email"]);
                assert_eq!(c.index_type, IndexType::BTree);
            }
            _ => panic!("Expected CreateIndex variant"),
        }
    }
}
