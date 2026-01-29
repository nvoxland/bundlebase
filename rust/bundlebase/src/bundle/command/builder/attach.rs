//! Attach command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::extract_string_content;
use crate::bundle::operation::AttachBlockOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to attach a data block to the bundle.
#[derive(Debug, Clone)]
pub struct AttachCommand {
    /// The path/URL of the data to attach
    pub path: String,
    /// The pack to attach to (None or "base" for base pack, otherwise join name)
    pub pack: Option<String>,
}

impl AttachCommand {
    /// Create a new AttachCommand.
    pub fn new(path: impl Into<String>, pack: Option<String>) -> Self {
        Self {
            path: path.into(),
            pack,
        }
    }
}

impl CommandParsing for AttachCommand {
    fn rule() -> Rule {
        Rule::attach_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut pack = None;
        let raw = pair.as_str().to_string();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::quoted_string => {
                    if path.is_none() {
                        path = Some(extract_string_content(inner_pair.as_str())?);
                    }
                }
                Rule::identifier => {
                    // The identifier after TO is the pack name
                    if pack.is_none() {
                        pack = Some(inner_pair.as_str().to_string());
                    }
                }
                Rule::with_options => {
                    // WITH options - not used yet
                }
                _ => {}
            }
        }

        // If pack wasn't captured from inner pairs, try to extract from raw string
        if pack.is_none() {
            let upper = raw.to_uppercase();
            if let Some(to_pos) = upper.find(" TO ") {
                let after_to = raw[to_pos + 4..].trim_start();
                let pack_name: String = after_to
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !pack_name.is_empty() {
                    pack = Some(pack_name);
                }
            }
        }

        let path = path.ok_or_else(|| -> BundlebaseError {
            "ATTACH statement missing path".into()
        })?;

        Ok(AttachCommand::new(path, pack))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        match &self.pack {
            Some(pack) if pack != "base" => {
                format!("ATTACH {} TO {}", escape_string(&self.path), pack)
            }
            _ => format!("ATTACH {}", escape_string(&self.path)),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for AttachCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;
        let pack_name = self.pack.as_deref().unwrap_or("base");

        let op = AttachBlockOp::setup(&pack_id, &self.path, builder).await?;
        builder.apply_operation(op.into()).await?;

        Ok(format!("Attached {} to {}", self.path, pack_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_attach_simple() {
        let input = "ATTACH 'data.parquet'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "data.parquet");
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_with_pack() {
        let input = "ATTACH 'more_users.parquet' TO users";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "more_users.parquet");
                assert_eq!(c.pack, Some("users".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = AttachCommand::new("data.csv", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "ATTACH 'data.csv'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "data.csv");
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_round_trip_with_pack() {
        let cmd = AttachCommand::new("orders.parquet", Some("orders".to_string()));
        let statement = cmd.to_statement();
        assert_eq!(statement, "ATTACH 'orders.parquet' TO orders");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Attach(c) => {
                assert_eq!(c.path, "orders.parquet");
                assert_eq!(c.pack, Some("orders".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }
}
