//! Set config command implementation (runtime-only).
//!
//! SetConfigCommand is a facade command that sets a configuration value
//! for the current session only. It does not persist the value to the bundle
//! manifest. It takes the highest priority, overriding even passed config.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::bundle_config::{ConfigKey, Scope};
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use log::info;
use std::sync::Arc;

/// Command to set a runtime config value (session-only, highest priority).
///
/// Unlike `SaveConfigCommand` which persists config to the bundle manifest,
/// `SetConfigCommand` only sets the value for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via the `BundleFacade` trait.
#[derive(Debug, Clone)]
pub struct SetConfigCommand {
    /// Configuration key
    pub key: String,
    /// Configuration value
    pub value: String,
    /// Named scope (e.g., "s3", "s3/bucket")
    pub scope: Scope,
}

impl SetConfigCommand {
    /// Create a new SetConfigCommand.
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

    /// Returns the Arrow schema for set config output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    /// Returns the expected output shape for set config output.
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for SetConfigCommand {
    fn rule() -> Rule {
        Rule::set_config_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut key = None;
        let mut value = None;
        let mut scope = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::identifier => {
                    if key.is_none() {
                        key = Some(inner_pair.as_str().to_string());
                    }
                }
                Rule::quoted_string => {
                    let s = crate::bundle::command::parser::extract_string_content(
                        inner_pair.as_str(),
                    )?;
                    if value.is_none() {
                        value = Some(s);
                    } else if scope.is_none() {
                        // Second quoted string is scope
                        scope = Some(s);
                    }
                }
                _ => {}
            }
        }

        let key =
            key.ok_or_else(|| BundlebaseError::from("SET CONFIG statement missing key"))?;
        let value =
            value.ok_or_else(|| BundlebaseError::from("SET CONFIG statement missing value"))?;
        let scope = match scope {
            Some(s) => Scope::from_path(&s)?,
            None => {
                return Err(BundlebaseError::from(
                    "SET CONFIG requires a FOR clause with a named scope",
                ));
            }
        };

        Ok(SetConfigCommand {
            key,
            value,
            scope,
        })
    }

    fn to_statement(&self) -> String {
        format!(
            "SET CONFIG {} = {} FOR {}",
            self.key,
            crate::bundle::command::parser::escape_string(&self.value),
            crate::bundle::command::parser::escape_string(self.scope.as_str())
        )
    }
}

#[async_trait]
impl BundleFacadeCommand for SetConfigCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        facade.set_config(
            &self.key,
            &self.value,
            &self.scope,
        )?;

        let specs = crate::all_config_specs();
        let is_secure = ConfigKey::is_key_secure(&self.key, &specs);
        let display_value = if is_secure {
            "'*****'".to_string()
        } else {
            crate::bundle::command::parser::escape_string(&self.value)
        };

        let display_statement = format!("SET CONFIG {} = {}", self.key, display_value);
        info!("Set runtime config: {}", display_statement);
        Ok(format!("OK: {}", display_statement))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_set_config_without_for_fails() {
        let input = "SET CONFIG region = 'us-west-2'";
        assert!(parse_command(input).is_err(), "SET CONFIG without FOR should fail");
    }

    #[test]
    fn test_parse_set_config_with_scope() {
        let input = "SET CONFIG region = 'us-west-2' FOR 's3/my-bucket'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetConfig(ref c) => {
                assert_eq!(c.key, "region");
                assert_eq!(c.value, "us-west-2");
                assert_eq!(c.scope, Scope::from_name("s3/my-bucket").unwrap());
            }
            _ => panic!("Expected SetConfig variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_round_trip_with_scope() {
        let cmd = SetConfigCommand::new(
            "endpoint",
            "http://localhost:9000",
            Scope::from_name("s3/test").unwrap(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "SET CONFIG endpoint = 'http://localhost:9000' FOR 's3/test'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetConfig(ref c) => {
                assert_eq!(c.key, "endpoint");
                assert_eq!(c.value, "http://localhost:9000");
                assert_eq!(c.scope, Scope::from_name("s3/test").unwrap());
            }
            _ => panic!("Expected SetConfig variant"),
        }
    }

    #[test]
    fn test_round_trip_named_scope() {
        let cmd = SetConfigCommand::new("region", "us-west-2", Scope::from_name("s3").unwrap());
        let statement = cmd.to_statement();
        assert_eq!(statement, "SET CONFIG region = 'us-west-2' FOR 's3'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetConfig(ref c) => {
                assert_eq!(c.key, "region");
                assert_eq!(c.value, "us-west-2");
                assert_eq!(c.scope, Scope::from_name("s3").unwrap());
            }
            _ => panic!("Expected SetConfig variant"),
        }
    }
}
