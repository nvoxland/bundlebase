use crate::parser::{extract_identifier, extract_string_content, quote_identifier};
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::CreateReportOp;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to create (or replace) a stored report.
#[derive(Debug, Clone)]
pub struct CreateReportCommand {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
}

impl CreateReportCommand {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            content: content.into(),
        }
    }
}

impl CommandParsing for CreateReportCommand {
    fn rule() -> Rule {
        Rule::create_report_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut id = None;
        let mut name = None;
        let mut description = None;
        let mut content = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::report_id => {
                    id = Some(extract_identifier(&inner));
                }
                Rule::quoted_string => {
                    let val = extract_string_content(inner.as_str())?;
                    if name.is_none() {
                        name = Some(val);
                    } else if description.is_none() {
                        description = Some(val);
                    } else {
                        content = Some(val);
                    }
                }
                _ => {}
            }
        }

        let id = id.ok_or_else(|| BundlebaseError::from("CREATE REPORT missing id"))?;
        let name = name.ok_or_else(|| BundlebaseError::from("CREATE REPORT missing NAME"))?;
        let description =
            description.ok_or_else(|| BundlebaseError::from("CREATE REPORT missing DESCRIPTION"))?;
        let content =
            content.ok_or_else(|| BundlebaseError::from("CREATE REPORT missing CONTENT"))?;

        Ok(CreateReportCommand::new(id, name, description, content))
    }

    fn to_statement(&self) -> String {
        format!(
            "CREATE REPORT {} NAME '{}' DESCRIPTION '{}' CONTENT $${}$$",
            quote_identifier(&self.id),
            self.name.replace('\'', "''"),
            self.description.replace('\'', "''"),
            self.content,
        )
    }
}

impl BundleBuilderCommand for CreateReportCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = CreateReportOp::setup(
            self.id.clone(),
            self.name.clone(),
            self.description.clone(),
            self.content.clone(),
            builder,
        ).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Created report: {}", self.id))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_create_report() {
        let input = "CREATE REPORT monthly-sales NAME 'Monthly Sales' DESCRIPTION 'A report' CONTENT $$# Hello$$";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateReport(c) => {
                assert_eq!(c.id, "monthly-sales");
                assert_eq!(c.name, "Monthly Sales");
                assert_eq!(c.description, "A report");
                assert_eq!(c.content, "# Hello");
            }
            _ => panic!("Expected CreateReport variant"),
        }
    }

    #[test]
    fn test_parse_create_report_quoted_id() {
        let input =
            "CREATE REPORT \"my report\" NAME 'Test' DESCRIPTION 'desc' CONTENT $$content$$";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateReport(c) => {
                assert_eq!(c.id, "my report");
            }
            _ => panic!("Expected CreateReport variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = CreateReportCommand::new("test-report", "Test", "A test report", "# Content");
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateReport(c) => {
                assert_eq!(c.id, "test-report");
                assert_eq!(c.name, "Test");
                assert_eq!(c.description, "A test report");
                assert_eq!(c.content, "# Content");
            }
            _ => panic!("Expected CreateReport variant"),
        }
    }
}
