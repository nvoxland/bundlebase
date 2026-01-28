//! SetConfig command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::SetConfigOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to set a configuration value.
#[derive(Debug, Clone)]
pub struct SetConfigCommand {
    /// Configuration key
    pub key: String,
    /// Configuration value
    pub value: String,
    /// Optional URL prefix for URL-specific config
    pub url_prefix: Option<String>,
}

impl SetConfigCommand {
    /// Create a new SetConfigCommand.
    pub fn new(
        key: impl Into<String>,
        value: impl Into<String>,
        url_prefix: Option<String>,
    ) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            url_prefix,
        }
    }
}

impl CommandParsing for SetConfigCommand {
    fn rule() -> Rule {
        Rule::set_config_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut key = None;
        let mut value = None;
        let mut url_prefix = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    if key.is_none() {
                        key = Some(inner.as_str().to_string());
                    }
                }
                Rule::quoted_string => {
                    if value.is_none() {
                        value = Some(extract_string_content(inner.as_str())?);
                    } else if url_prefix.is_none() {
                        url_prefix = Some(extract_string_content(inner.as_str())?);
                    }
                }
                _ => {}
            }
        }

        let key = key.ok_or_else(|| -> BundlebaseError { "SET CONFIG missing key".into() })?;
        let value = value.ok_or_else(|| -> BundlebaseError { "SET CONFIG missing value".into() })?;

        Ok(SetConfigCommand::new(key, value, url_prefix))
    }

    fn to_statement(&self) -> String {
        match &self.url_prefix {
            Some(prefix) => format!(
                "SET CONFIG {} = {} FOR {}",
                self.key,
                escape_string(&self.value),
                escape_string(prefix)
            ),
            None => format!("SET CONFIG {} = {}", self.key, escape_string(&self.value)),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for SetConfigCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = SetConfigOp::setup(&self.key, &self.value, self.url_prefix.as_deref());
        builder.apply_operation(op.into()).await?;
        Ok(format!("Set config: {} = {}", self.key, self.value))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_set_config() {
        let input = "SET CONFIG timeout = '30'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetConfig(c) => {
                assert_eq!(c.key, "timeout");
                assert_eq!(c.value, "30");
                assert_eq!(c.url_prefix, None);
            }
            _ => panic!("Expected SetConfig variant"),
        }
    }

    #[test]
    fn test_parse_set_config_with_prefix() {
        let input = "SET CONFIG access_key = 'secret123' FOR 's3://'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetConfig(c) => {
                assert_eq!(c.key, "access_key");
                assert_eq!(c.value, "secret123");
                assert_eq!(c.url_prefix, Some("s3://".to_string()));
            }
            _ => panic!("Expected SetConfig variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = SetConfigCommand::new("region", "us-east-1", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "SET CONFIG region = 'us-east-1'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetConfig(c) => {
                assert_eq!(c.key, "region");
                assert_eq!(c.value, "us-east-1");
            }
            _ => panic!("Expected SetConfig variant"),
        }
    }

    #[test]
    fn test_round_trip_with_prefix() {
        let cmd = SetConfigCommand::new("bucket", "my-bucket", Some("s3://".to_string()));
        let statement = cmd.to_statement();
        assert_eq!(statement, "SET CONFIG bucket = 'my-bucket' FOR 's3://'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetConfig(c) => {
                assert_eq!(c.key, "bucket");
                assert_eq!(c.value, "my-bucket");
                assert_eq!(c.url_prefix, Some("s3://".to_string()));
            }
            _ => panic!("Expected SetConfig variant"),
        }
    }
}
