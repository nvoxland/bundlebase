//! SetDescription command implementation.

use crate::{CommandParsing, Rule};
use crate::parser::{escape_string, extract_string_content};
use bundlebase::bundle::operation::SetDescriptionOp;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to set the bundle's description.
#[derive(Debug, Clone)]
pub struct SetDescriptionCommand {
    /// The description to set
    pub description: String,
}

impl SetDescriptionCommand {
    /// Create a new SetDescriptionCommand.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
        }
    }
}

impl CommandParsing for SetDescriptionCommand {
    fn rule() -> Rule {
        Rule::set_description_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut description = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                description = Some(extract_string_content(inner.as_str())?);
            }
        }

        let description = description.ok_or_else(|| -> BundlebaseError {
            "SET DESCRIPTION missing description".into()
        })?;

        Ok(SetDescriptionCommand::new(description))
    }

    fn to_statement(&self) -> String {
        format!("SET DESCRIPTION {}", escape_string(&self.description))
    }
}

impl BundleBuilderCommand for SetDescriptionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(SetDescriptionOp::setup(&self.description).into())
            .await?;
        Ok("Set bundle description".to_string())
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_set_description() {
        let input = "SET DESCRIPTION 'A test bundle'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetDescription(c) => {
                assert_eq!(c.description, "A test bundle");
            }
            _ => panic!("Expected SetDescription variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = SetDescriptionCommand::new("My bundle description");
        let statement = cmd.to_statement();
        assert_eq!(statement, "SET DESCRIPTION 'My bundle description'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetDescription(c) => {
                assert_eq!(c.description, "My bundle description");
            }
            _ => panic!("Expected SetDescription variant"),
        }
    }
}
