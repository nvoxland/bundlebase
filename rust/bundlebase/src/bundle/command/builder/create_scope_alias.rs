//! CreateScopeAlias command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::CreateScopeAliasOp;
use crate::bundle_config::Scope;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to create a named scope alias (name -> scope mapping).
#[derive(Debug, Clone)]
pub struct CreateScopeAliasCommand {
    /// Alias name
    pub name: String,
    /// Scope this alias maps to
    pub scope: Scope,
}

impl CreateScopeAliasCommand {
    /// Create a new CreateScopeAliasCommand.
    pub fn new(name: impl Into<String>, scope: Scope) -> Self {
        Self {
            name: name.into(),
            scope,
        }
    }
}

impl CommandParsing for CreateScopeAliasCommand {
    fn rule() -> Rule {
        Rule::create_scope_alias_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut scope = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    if name.is_none() {
                        name = Some(inner.as_str().to_string());
                    }
                }
                Rule::quoted_string => {
                    if scope.is_none() {
                        scope = Some(extract_string_content(inner.as_str())?);
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "CREATE SCOPE ALIAS missing name".into()
        })?;
        let scope = scope.ok_or_else(|| -> BundlebaseError {
            "CREATE SCOPE ALIAS missing scope".into()
        })?;

        Ok(CreateScopeAliasCommand::new(name, Scope::from_url(&scope)))
    }

    fn to_statement(&self) -> String {
        format!(
            "CREATE SCOPE ALIAS {} AS {}",
            self.name,
            escape_string(self.scope.as_str())
        )
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateScopeAliasCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = CreateScopeAliasOp::setup(&self.name, &self.scope);
        builder.apply_operation(op.into()).await?;
        Ok(format!(
            "Created scope alias: {} = {}",
            self.name, self.scope
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_scope_alias() {
        let input = "CREATE SCOPE ALIAS prod AS 's3://my-bucket/'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateScopeAlias(c) => {
                assert_eq!(c.name, "prod");
                assert_eq!(c.scope, Scope::from_url("s3://my-bucket/"));
            }
            _ => panic!("Expected CreateScopeAlias variant"),
        }
    }

    #[test]
    fn test_parse_create_scope_alias_round_trip() {
        let input = "CREATE SCOPE ALIAS staging AS 'gs://staging-bucket/data/'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateScopeAlias(c) => {
                let statement = c.to_statement();
                let reparsed = parse_command(&statement).unwrap();
                match reparsed {
                    BundleCommand::CreateScopeAlias(c2) => {
                        assert_eq!(c2.name, "staging");
                        assert_eq!(c2.scope, Scope::from_url("gs://staging-bucket/data/"));
                    }
                    _ => panic!("Expected CreateScopeAlias variant on reparse"),
                }
            }
            _ => panic!("Expected CreateScopeAlias variant"),
        }
    }

    #[test]
    fn test_to_statement() {
        let cmd = CreateScopeAliasCommand::new("prod", Scope::from_url("s3://my-bucket/"));
        assert_eq!(
            cmd.to_statement(),
            "CREATE SCOPE ALIAS prod AS '/s3/my-bucket'"
        );
    }
}
