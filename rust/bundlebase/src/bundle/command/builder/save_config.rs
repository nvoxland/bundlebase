//! SaveConfig command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::SaveConfigOp;
use crate::bundle_config::{ConfigKey, Scope};
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to save a configuration value to the bundle manifest.
///
/// Supports scoped config:
/// - `SAVE CONFIG key = 'value' FOR 'scope'` -- scope path (e.g., 's3/bucket')
#[derive(Debug, Clone)]
pub struct SaveConfigCommand {
    /// Configuration key
    pub key: String,
    /// Configuration value
    pub value: String,
    /// Named scope (e.g., "s3", "s3/bucket")
    pub scope: Scope,
}

impl SaveConfigCommand {
    /// Create a new SaveConfigCommand.
    pub fn new(
        key: impl Into<String>,
        value: impl Into<String>,
        scope: Scope,
    ) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            scope,
        }
    }
}

impl CommandParsing for SaveConfigCommand {
    fn rule() -> Rule {
        Rule::save_config_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut key = None;
        let mut value = None;
        let mut scope = None;

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
                    } else if scope.is_none() {
                        // Second quoted_string is the scope (from FOR '<scope>')
                        scope = Some(extract_string_content(inner.as_str())?);
                    }
                }
                _ => {}
            }
        }

        let key = key.ok_or_else(|| -> BundlebaseError { "SAVE CONFIG missing key".into() })?;
        let value = value.ok_or_else(|| -> BundlebaseError { "SAVE CONFIG missing value".into() })?;
        let scope = match scope {
            Some(s) => Scope::from_path(&s)?,
            None => {
                return Err("SAVE CONFIG requires a FOR clause with a named scope".into());
            }
        };

        Ok(SaveConfigCommand {
            key,
            value,
            scope,
        })
    }

    fn to_statement(&self) -> String {
        format!(
            "SAVE CONFIG {} = {} FOR {}",
            self.key,
            escape_string(&self.value),
            escape_string(self.scope.as_str())
        )
    }
}

#[async_trait]
impl BundleBuilderCommand for SaveConfigCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = SaveConfigOp::setup(&self.key, &self.value, &self.scope);
        builder.apply_operation(op.into()).await?;

        let specs = crate::all_config_specs();
        let display_value = if ConfigKey::is_key_secure(&self.key, &specs) {
            "*****".to_string()
        } else {
            self.value.clone()
        };
        Ok(format!("Saved config: {} = {}", self.key, display_value))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_save_config_without_for_fails() {
        let input = "SAVE CONFIG timeout = '30'";
        assert!(parse_command(input).is_err(), "SAVE CONFIG without FOR should fail");
    }

    #[test]
    fn test_parse_save_config_with_scope() {
        let input = "SAVE CONFIG access_key = 'secret123' FOR 's3'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SaveConfig(c) => {
                assert_eq!(c.key, "access_key");
                assert_eq!(c.value, "secret123");
                assert_eq!(c.scope, Scope::new("s3"));
            }
            _ => panic!("Expected SaveConfig variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = SaveConfigCommand::new("region", "us-east-1", Scope::new("s3"));
        let statement = cmd.to_statement();
        assert_eq!(statement, "SAVE CONFIG region = 'us-east-1' FOR 's3'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SaveConfig(c) => {
                assert_eq!(c.key, "region");
                assert_eq!(c.value, "us-east-1");
                assert_eq!(c.scope, Scope::new("s3"));
            }
            _ => panic!("Expected SaveConfig variant"),
        }
    }

    #[test]
    fn test_round_trip_with_scope() {
        let cmd = SaveConfigCommand::new("bucket", "my-bucket", Scope::new("s3"));
        let statement = cmd.to_statement();
        assert_eq!(statement, "SAVE CONFIG bucket = 'my-bucket' FOR 's3'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SaveConfig(c) => {
                assert_eq!(c.key, "bucket");
                assert_eq!(c.value, "my-bucket");
                assert_eq!(c.scope, Scope::new("s3"));
            }
            _ => panic!("Expected SaveConfig variant"),
        }
    }
}
