//! SaveConfig command implementation.

use crate::{CommandParsing, Rule};
use crate::parser::{escape_string, extract_string_content};
use bundlebase::bundle::operation::SaveConfigOp;
use bundlebase::bundle_config::Scope;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

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
        scope: Scope,
        key: impl Into<String>,
        value: impl Into<String>,
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
            Some(s) => Scope::try_from(s.as_str())?,
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

impl BundleBuilderCommand for SaveConfigCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = SaveConfigOp::setup(&self.scope, &self.key, &self.value);
        builder.apply_operation(op.into()).await?;

        let display_value = if bundlebase::bundle_config::is_key_secure(&self.scope, &self.key) {
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
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

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
                assert_eq!(c.scope, Scope::try_from("s3").unwrap());
            }
            _ => panic!("Expected SaveConfig variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = SaveConfigCommand::new(Scope::try_from("s3").unwrap(), "region", "us-east-1");
        let statement = cmd.to_statement();
        assert_eq!(statement, "SAVE CONFIG region = 'us-east-1' FOR 's3'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SaveConfig(c) => {
                assert_eq!(c.key, "region");
                assert_eq!(c.value, "us-east-1");
                assert_eq!(c.scope, Scope::try_from("s3").unwrap());
            }
            _ => panic!("Expected SaveConfig variant"),
        }
    }

    #[test]
    fn test_round_trip_with_scope() {
        let cmd = SaveConfigCommand::new(Scope::try_from("s3").unwrap(), "bucket", "my-bucket");
        let statement = cmd.to_statement();
        assert_eq!(statement, "SAVE CONFIG bucket = 'my-bucket' FOR 's3'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SaveConfig(c) => {
                assert_eq!(c.key, "bucket");
                assert_eq!(c.value, "my-bucket");
                assert_eq!(c.scope, Scope::try_from("s3").unwrap());
            }
            _ => panic!("Expected SaveConfig variant"),
        }
    }
}
