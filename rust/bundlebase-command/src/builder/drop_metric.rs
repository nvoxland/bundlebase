//! DropMetric command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::DropMetricOp;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to remove a named metric from the bundle.
/// Syntax: `DROP METRIC <name>`.
#[derive(Debug, Clone)]
pub struct DropMetricCommand {
    pub name: String,
}

impl DropMetricCommand {
    /// Create a new DropMetricCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for DropMetricCommand {
    fn rule() -> Rule {
        Rule::drop_metric_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                name = Some(extract_identifier(&inner));
            }
        }
        let name =
            name.ok_or_else(|| -> BundlebaseError { "DROP METRIC missing metric name".into() })?;
        Ok(DropMetricCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP METRIC {}", quote_identifier(&self.name))
    }
}

impl BundleBuilderCommand for DropMetricCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(DropMetricOp::setup(&self.name).into())
            .await?;
        Ok(format!("Dropped metric: {}", self.name))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test setup and assertions may panic on failure.
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_drop_metric() {
        let cmd = parse_command("DROP METRIC total_revenue").unwrap();
        match cmd {
            BundleCommand::DropMetric(c) => assert_eq!(c.name, "total_revenue"),
            _ => panic!("Expected DropMetric variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropMetricCommand::new("aov");
        let parsed = parse_command(&cmd.to_statement()).unwrap();
        match parsed {
            BundleCommand::DropMetric(c) => assert_eq!(c.name, "aov"),
            _ => panic!("Expected DropMetric variant"),
        }
    }
}
