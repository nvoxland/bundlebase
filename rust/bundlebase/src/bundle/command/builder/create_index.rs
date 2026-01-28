//! CreateIndex command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::CreateIndexOp;
use crate::index::IndexType;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to create an index on a column.
#[derive(Debug, Clone)]
pub struct CreateIndexCommand {
    /// The column to index
    pub column: String,
    /// The type of index to create
    pub index_type: IndexType,
}

impl CreateIndexCommand {
    /// Create a new CreateIndexCommand.
    pub fn new(column: impl Into<String>, index_type: IndexType) -> Self {
        Self {
            column: column.into(),
            index_type,
        }
    }
}

impl CommandParsing for CreateIndexCommand {
    fn rule() -> Rule {
        Rule::create_index_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut column = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                column = Some(inner.as_str().to_string());
            }
        }

        let column = column.ok_or_else(|| -> BundlebaseError {
            "CREATE INDEX statement missing column name".into()
        })?;

        // Default to column index type
        Ok(CreateIndexCommand::new(column, IndexType::Column))
    }

    fn to_statement(&self) -> String {
        match &self.index_type {
            IndexType::Column => format!("CREATE INDEX ON {}", self.column),
            IndexType::Text { tokenizer } => {
                format!("CREATE TEXT INDEX ON {} (tokenizer: {:?})", self.column, tokenizer)
            }
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateIndexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(
                CreateIndexOp::setup(&self.column, self.index_type.clone())
                    .await?
                    .into(),
            )
            .await?;

        builder.reindex_internal().await?;

        Ok(format!("Created index on column: {}", self.column))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_index() {
        let input = "CREATE INDEX ON user_id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateIndex(c) => {
                assert_eq!(c.column, "user_id");
            }
            _ => panic!("Expected CreateIndex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = CreateIndexCommand::new("email", IndexType::Column);
        let statement = cmd.to_statement();
        assert_eq!(statement, "CREATE INDEX ON email");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateIndex(c) => {
                assert_eq!(c.column, "email");
            }
            _ => panic!("Expected CreateIndex variant"),
        }
    }
}
