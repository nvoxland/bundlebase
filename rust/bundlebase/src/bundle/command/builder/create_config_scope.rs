//! CreateConfigScope command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::CreateConfigScopeOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to create a named config scope (name -> URL mapping).
#[derive(Debug, Clone)]
pub struct CreateConfigScopeCommand {
    /// Scope name
    pub name: String,
    /// URL prefix this scope maps to
    pub url: String,
}

impl CreateConfigScopeCommand {
    /// Create a new CreateConfigScopeCommand.
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
        }
    }
}

impl CommandParsing for CreateConfigScopeCommand {
    fn rule() -> Rule {
        Rule::create_config_scope_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut url = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    if name.is_none() {
                        name = Some(inner.as_str().to_string());
                    }
                }
                Rule::quoted_string => {
                    if url.is_none() {
                        url = Some(extract_string_content(inner.as_str())?);
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "CREATE CONFIG SCOPE missing name".into()
        })?;
        let url = url.ok_or_else(|| -> BundlebaseError {
            "CREATE CONFIG SCOPE missing URL".into()
        })?;

        Ok(CreateConfigScopeCommand::new(name, url))
    }

    fn to_statement(&self) -> String {
        format!(
            "CREATE CONFIG SCOPE {} AS {}",
            self.name,
            escape_string(&self.url)
        )
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateConfigScopeCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = CreateConfigScopeOp::setup(&self.name, &self.url);
        builder.apply_operation(op.into()).await?;
        Ok(format!(
            "Created config scope: {} = {}",
            self.name, self.url
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_config_scope() {
        let input = "CREATE CONFIG SCOPE prod AS 's3://my-bucket/'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConfigScope(c) => {
                assert_eq!(c.name, "prod");
                assert_eq!(c.url, "s3://my-bucket/");
            }
            _ => panic!("Expected CreateConfigScope variant"),
        }
    }

    #[test]
    fn test_parse_create_config_scope_round_trip() {
        let input = "CREATE CONFIG SCOPE staging AS 'gs://staging-bucket/data/'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConfigScope(c) => {
                let statement = c.to_statement();
                let reparsed = parse_command(&statement).unwrap();
                match reparsed {
                    BundleCommand::CreateConfigScope(c2) => {
                        assert_eq!(c2.name, "staging");
                        assert_eq!(c2.url, "gs://staging-bucket/data/");
                    }
                    _ => panic!("Expected CreateConfigScope variant on reparse"),
                }
            }
            _ => panic!("Expected CreateConfigScope variant"),
        }
    }

    #[test]
    fn test_to_statement() {
        let cmd = CreateConfigScopeCommand::new("prod", "s3://my-bucket/");
        assert_eq!(
            cmd.to_statement(),
            "CREATE CONFIG SCOPE prod AS 's3://my-bucket/'"
        );
    }
}
