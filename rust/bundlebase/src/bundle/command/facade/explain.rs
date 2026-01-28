//! Explain plan command implementation.
//!
//! ExplainPlanCommand is a facade command - it works with `BundleFacade::explain()` to
//! return the query execution plan. It does not mutate the source bundle.

use crate::bundle::command::{CommandParsing, Rule};
use crate::BundlebaseError;

// ============================================================================
// ExplainPlanCommand
// ============================================================================

/// Command to show the query execution plan.
///
/// ExplainPlanCommand is executed via `BundleFacade.explain()` which returns a
/// formatted string describing the query execution plan.
#[derive(Debug, Clone)]
pub struct ExplainPlanCommand;

impl ExplainPlanCommand {
    /// Create a new ExplainPlanCommand.
    pub fn new() -> Self {
        Self
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

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(ExplainPlanCommand)
    }

    fn to_statement(&self) -> String {
        "EXPLAIN PLAN".to_string()
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_explain_plan() {
        let input = "EXPLAIN PLAN";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ExplainPlan(_) => {}
            _ => panic!("Expected ExplainPlan variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ExplainPlanCommand::new();
        let statement = cmd.to_statement();
        assert_eq!(statement, "EXPLAIN PLAN");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ExplainPlan(_) => {}
            _ => panic!("Expected ExplainPlan variant"),
        }
    }
}
