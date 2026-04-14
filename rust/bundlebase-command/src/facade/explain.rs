//! Explain plan command implementation.
//!
//! ExplainPlanCommand is a facade command that computes and returns the query
//! execution plan. It does not mutate the source bundle.

use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::logical_expr::ExplainOption;
use std::sync::Arc;

// ============================================================================
// ExplainPlanCommand
// ============================================================================

/// Command to show the query execution plan.
///
/// ExplainPlanCommand works with `BundleFacade` to compute and return the query
/// execution plan as a stream. It does not mutate the bundle.
#[derive(Debug, Clone)]
pub struct ExplainPlanCommand {
    pub analyze: bool,
    pub verbose: bool,
    pub format: Option<String>,
    pub sql: Option<String>,
}

impl ExplainPlanCommand {
    /// Create a new ExplainPlanCommand with defaults.
    pub fn new() -> Self {
        Self {
            analyze: false,
            verbose: false,
            format: None,
            sql: None,
        }
    }

    /// Returns the Arrow schema for explain output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("plan_type", DataType::Utf8, false),
            Field::new("plan", DataType::Utf8, false),
        ]))
    }

    /// Returns the expected output shape for explain output.
    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    /// Convert the format string to a DataFusion ExplainFormat.
    pub fn to_explain_format(&self) -> datafusion::logical_expr::ExplainFormat {
        match self.format.as_deref() {
            Some("TREE") => datafusion::logical_expr::ExplainFormat::Tree,
            Some("GRAPHVIZ") => datafusion::logical_expr::ExplainFormat::Graphviz,
            _ => datafusion::logical_expr::ExplainFormat::Indent,
        }
    }
}

impl Default for ExplainPlanCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandParsing for ExplainPlanCommand {
    fn rule() -> Rule {
        Rule::explain_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut analyze = false;
        let mut verbose = false;
        let mut format = None;
        let mut sql = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::explain_analyze => {
                    analyze = true;
                }
                Rule::explain_verbose => {
                    verbose = true;
                }
                Rule::explain_format => {
                    format = Some(inner.as_str().to_uppercase());
                }
                Rule::explain_sql => {
                    sql = Some(inner.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        Ok(ExplainPlanCommand {
            analyze,
            verbose,
            format,
            sql,
        })
    }

    fn to_statement(&self) -> String {
        let mut parts = vec!["EXPLAIN".to_string()];
        if self.analyze {
            parts.push("ANALYZE".to_string());
        }
        if self.verbose {
            parts.push("VERBOSE".to_string());
        }
        if let Some(ref fmt) = self.format {
            parts.push(format!("FORMAT {}", fmt.to_uppercase()));
        }
        if let Some(ref sql) = self.sql {
            parts.push(sql.clone());
        }
        parts.join(" ")
    }
}

impl BundleFacadeCommand for ExplainPlanCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let df = match self.sql.as_deref() {
            Some(sql) => {
                let ctx = facade.ctx();
                let plan = ctx.state().create_logical_plan(sql).await?;
                ctx.execute_logical_plan(plan).await?
            }
            None => (*facade.dataframe().await?).clone(),
        };
        let plan_df = df.explain_with_options(ExplainOption {
            verbose: self.verbose,
            analyze: self.analyze,
            format: self.to_explain_format(),
        })?;
        Ok(plan_df.execute_stream().await?)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_explain() {
        let input = "EXPLAIN";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(!c.analyze);
                assert!(!c.verbose);
                assert!(c.format.is_none());
                assert!(c.sql.is_none());
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_analyze() {
        let input = "EXPLAIN ANALYZE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(c.analyze);
                assert!(!c.verbose);
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_verbose() {
        let input = "EXPLAIN VERBOSE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(!c.analyze);
                assert!(c.verbose);
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_analyze_verbose() {
        let input = "EXPLAIN ANALYZE VERBOSE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(c.analyze);
                assert!(c.verbose);
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_format_tree() {
        let input = "EXPLAIN FORMAT TREE";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert_eq!(c.format.as_deref(), Some("TREE"));
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_analyze_verbose_format_graphviz() {
        let input = "EXPLAIN ANALYZE VERBOSE FORMAT GRAPHVIZ";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(c.analyze);
                assert!(c.verbose);
                assert_eq!(c.format.as_deref(), Some("GRAPHVIZ"));
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_with_sql() {
        let input = "EXPLAIN SELECT * FROM bundle";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(!c.analyze);
                assert!(!c.verbose);
                assert!(c.format.is_none());
                assert_eq!(c.sql.as_deref(), Some("SELECT * FROM bundle"));
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_parse_explain_analyze_format_tree_with_sql() {
        let input = "EXPLAIN ANALYZE FORMAT TREE SELECT * FROM bundle WHERE id > 10";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(c.analyze);
                assert!(!c.verbose);
                assert_eq!(c.format.as_deref(), Some("TREE"));
                assert_eq!(c.sql.as_deref(), Some("SELECT * FROM bundle WHERE id > 10"));
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_round_trip_basic() {
        let cmd = ExplainPlanCommand::new();
        let statement = cmd.to_statement();
        assert_eq!(statement, "EXPLAIN");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(!c.analyze);
                assert!(!c.verbose);
                assert!(c.format.is_none());
                assert!(c.sql.is_none());
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_round_trip_analyze_verbose() {
        let cmd = ExplainPlanCommand {
            analyze: true,
            verbose: true,
            format: None,
            sql: None,
        };
        let statement = cmd.to_statement();
        assert_eq!(statement, "EXPLAIN ANALYZE VERBOSE");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(c.analyze);
                assert!(c.verbose);
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_round_trip_with_format_and_sql() {
        let cmd = ExplainPlanCommand {
            analyze: true,
            verbose: false,
            format: Some("TREE".to_string()),
            sql: Some("SELECT * FROM bundle".to_string()),
        };
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "EXPLAIN ANALYZE FORMAT TREE SELECT * FROM bundle"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ExplainPlan(ref c) => {
                assert!(c.analyze);
                assert!(!c.verbose);
                assert_eq!(c.format.as_deref(), Some("TREE"));
                assert_eq!(c.sql.as_deref(), Some("SELECT * FROM bundle"));
            }
            _ => panic!("Expected ExplainPlan variant"),
        }
    }
}
