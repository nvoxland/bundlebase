use crate::parser::{extract_identifier, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::DropReportOp;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to drop a stored report.
#[derive(Debug, Clone)]
pub struct DropReportCommand {
    pub id: String,
}

impl DropReportCommand {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl CommandParsing for DropReportCommand {
    fn rule() -> Rule {
        Rule::drop_report_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut id = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::report_id {
                id = Some(extract_identifier(&inner));
            }
        }

        let id = id.ok_or_else(|| -> BundlebaseError { "DROP REPORT missing id".into() })?;
        Ok(DropReportCommand::new(id))
    }

    fn to_statement(&self) -> String {
        format!("DROP REPORT {}", quote_identifier(&self.id))
    }
}

impl BundleBuilderCommand for DropReportCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropReportOp::setup(&self.id, builder)?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Dropped report: {}", self.id))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_drop_report() {
        let input = "DROP REPORT monthly-sales";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropReport(c) => {
                assert_eq!(c.id, "monthly-sales");
            }
            _ => panic!("Expected DropReport variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropReportCommand::new("test-report");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP REPORT \"test-report\"");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropReport(c) => {
                assert_eq!(c.id, "test-report");
            }
            _ => panic!("Expected DropReport variant"),
        }
    }
}
