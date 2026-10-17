//! Declarative keys describe data; they do not enforce uniqueness.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{BundleBuilderCommand, CommandParsing, Rule};
use bundlebase::bundle::operation::{DeclareKeyOp, DropKeyOp};
use bundlebase::{BundleBuilder, BundleFacade};
use bundlebase_common::BundlebaseError;

fn parse_columns(pair: pest::iterators::Pair<Rule>) -> Vec<String> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::semantic_key_columns)
        .flat_map(|p| p.into_inner())
        .map(|p| extract_identifier(&p))
        .collect()
}

fn column_clause(columns: &[String]) -> String {
    columns
        .iter()
        .map(|s| quote_identifier(s))
        .collect::<Vec<_>>()
        .join(", ")
}

macro_rules! key_command {
    ($name:ident, $rule:ident, $prefix:literal, $op:ident, $unique:literal) => {
        #[derive(Debug, Clone)]
        pub struct $name {
            pub columns: Vec<String>,
        }
        impl CommandParsing for $name {
            fn rule() -> Rule {
                Rule::$rule
            }
            fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
                Ok(Self {
                    columns: parse_columns(pair),
                })
            }
            fn to_statement(&self) -> String {
                format!("{} ({})", $prefix, column_clause(&self.columns))
            }
        }
        impl BundleBuilderCommand for $name {
            type Output = String;
            async fn execute(
                self: Box<Self>,
                builder: &BundleBuilder,
            ) -> Result<String, BundlebaseError> {
                let mut columns = Vec::with_capacity(self.columns.len());
                for name in &self.columns {
                    columns.push(builder.column_id(name).ok_or_else(|| {
                        BundlebaseError::from(format!("Column '{}' not found", name))
                    })?);
                }
                builder
                    .apply_operation($op::setup(columns, $unique).into())
                    .await?;
                Ok($prefix.to_string())
            }
        }
    };
}

key_command!(
    DeclarePrimaryKeyCommand,
    declare_primary_key_stmt,
    "DECLARE PRIMARY KEY",
    DeclareKeyOp,
    false
);
key_command!(
    DeclareUniqueKeyCommand,
    declare_unique_key_stmt,
    "DECLARE UNIQUE KEY",
    DeclareKeyOp,
    true
);
key_command!(
    DropUniqueKeyCommand,
    drop_unique_key_stmt,
    "DROP UNIQUE KEY",
    DropKeyOp,
    true
);

#[derive(Debug, Clone)]
pub struct DropPrimaryKeyCommand;
impl CommandParsing for DropPrimaryKeyCommand {
    fn rule() -> Rule {
        Rule::drop_primary_key_stmt
    }
    fn from_statement(_: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(Self)
    }
    fn to_statement(&self) -> String {
        "DROP PRIMARY KEY".to_string()
    }
}
impl BundleBuilderCommand for DropPrimaryKeyCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(DropKeyOp::setup(Vec::new(), false).into())
            .await?;
        Ok("Dropped primary key".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser::parse_command, BundleCommand};

    #[test]
    fn keys_parse_and_round_trip() -> Result<(), BundlebaseError> {
        for sql in [
            "DECLARE PRIMARY KEY (id)",
            "DECLARE UNIQUE KEY (id, \"Order Date\")",
            "DROP UNIQUE KEY (id, \"Order Date\")",
            "DROP PRIMARY KEY",
        ] {
            let statement = match parse_command(sql)? {
                BundleCommand::DeclarePrimaryKey(c) => c.to_statement(),
                BundleCommand::DeclareUniqueKey(c) => {
                    assert_eq!(c.columns, ["id", "Order Date"]);
                    c.to_statement()
                }
                BundleCommand::DropUniqueKey(c) => c.to_statement(),
                BundleCommand::DropPrimaryKey(c) => c.to_statement(),
                _ => return Err("Expected semantic key command".into()),
            };
            parse_command(&statement)?;
        }
        assert!(parse_command("DECLARE PRIMARY KEY ()").is_err());
        assert!(parse_command("DROP UNIQUE KEY").is_err());
        Ok(())
    }
}
