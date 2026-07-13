//! DefineMetric command implementation.

use crate::parser::{extract_identifier, extract_string_content, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::DefineMetricOp;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to define a named metric: a reusable aggregate expression that
/// travels in the bundle and is surfaced to agents/tools so queries share one
/// canonical definition. Syntax:
/// `CREATE METRIC <name> AS '<expression>' [DESCRIPTION '<text>']`.
#[derive(Debug, Clone)]
pub struct DefineMetricCommand {
    pub name: String,
    pub expression: String,
    pub description: Option<String>,
}

impl DefineMetricCommand {
    /// Create a new DefineMetricCommand.
    pub fn new(
        name: impl Into<String>,
        expression: impl Into<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
            description,
        }
    }
}

impl CommandParsing for DefineMetricCommand {
    fn rule() -> Rule {
        Rule::create_metric_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        // The grammar yields the expression string first, then an optional
        // description string (the DESCRIPTION keyword itself is not captured).
        let mut strings: Vec<String> = Vec::new();

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => name = Some(extract_identifier(&inner)),
                Rule::quoted_string => strings.push(extract_string_content(inner.as_str())?),
                _ => {}
            }
        }

        let name = name
            .ok_or_else(|| -> BundlebaseError { "CREATE METRIC missing metric name".into() })?;
        let mut strings = strings.into_iter();
        let expression = strings
            .next()
            .ok_or_else(|| -> BundlebaseError { "CREATE METRIC missing expression".into() })?;
        let description = strings.next();

        Ok(DefineMetricCommand::new(name, expression, description))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        let mut out = format!(
            "CREATE METRIC {} AS {}",
            quote_identifier(&self.name),
            escape_string(&self.expression)
        );
        if let Some(desc) = &self.description {
            out.push_str(&format!(" DESCRIPTION {}", escape_string(desc)));
        }
        out
    }
}

impl BundleBuilderCommand for DefineMetricCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(
                DefineMetricOp::setup(&self.name, &self.expression, self.description.as_deref())
                    .into(),
            )
            .await?;
        Ok(format!("Defined metric: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_without_description() {
        let cmd = parse_command("CREATE METRIC total_revenue AS 'SUM(amount)'").unwrap();
        match cmd {
            BundleCommand::DefineMetric(c) => {
                assert_eq!(c.name, "total_revenue");
                assert_eq!(c.expression, "SUM(amount)");
                assert_eq!(c.description, None);
            }
            _ => panic!("Expected DefineMetric variant"),
        }
    }

    #[test]
    fn test_parse_with_description() {
        let cmd = parse_command(
            "CREATE METRIC aov AS 'SUM(amount) / COUNT(DISTINCT order_id)' DESCRIPTION 'average order value'",
        )
        .unwrap();
        match cmd {
            BundleCommand::DefineMetric(c) => {
                assert_eq!(c.name, "aov");
                assert_eq!(c.expression, "SUM(amount) / COUNT(DISTINCT order_id)");
                assert_eq!(c.description.as_deref(), Some("average order value"));
            }
            _ => panic!("Expected DefineMetric variant"),
        }
    }

    #[test]
    fn test_round_trip_with_description() {
        let cmd = DefineMetricCommand::new(
            "active_users",
            "COUNT(DISTINCT user_id)",
            Some("distinct active users".to_string()),
        );
        let parsed = parse_command(&cmd.to_statement()).unwrap();
        match parsed {
            BundleCommand::DefineMetric(c) => {
                assert_eq!(c.name, "active_users");
                assert_eq!(c.expression, "COUNT(DISTINCT user_id)");
                assert_eq!(c.description.as_deref(), Some("distinct active users"));
            }
            _ => panic!("Expected DefineMetric variant"),
        }
    }
}
