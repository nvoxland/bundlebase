//! ShowCount command implementation.
//!
//! `SHOW COUNT` returns the number of rows in the bundle.

use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use bundlebase::BundleFacade;
use crate::CommandResponse;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Command to show the row count of the bundle.
#[derive(Debug, Clone)]
pub struct ShowCountCommand;

impl ShowCountCommand {
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "count",
            DataType::Int64,
            false,
        )]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for ShowCountCommand {
    fn rule() -> Rule {
        Rule::show_count_stmt
    }

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(ShowCountCommand)
    }

    fn to_statement(&self) -> String {
        "SHOW COUNT".to_string()
    }
}

impl BundleFacadeCommand for ShowCountCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let count = facade.num_rows().await?;
        Box::new(count).into_stream()
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_show_count() {
        let cmd = parse_command("SHOW COUNT").expect("Failed to parse SHOW COUNT");
        assert!(
            matches!(cmd, BundleCommand::ShowCount(_)),
            "Expected ShowCount variant, got {:?}",
            cmd
        );
    }

    #[test]
    fn test_parse_show_count_case_insensitive() {
        let cmd = parse_command("show count").expect("Failed to parse show count");
        assert!(matches!(cmd, BundleCommand::ShowCount(_)));
    }
}
