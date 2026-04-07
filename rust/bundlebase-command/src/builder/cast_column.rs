//! CastColumn command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{CommandParsing, Rule};
use bundlebase_common::arrow_types::parse_arrow_type_name;
use bundlebase::bundle::operation::CastColumnOp;
use bundlebase::bundle::BundleFacade;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use futures::StreamExt;

/// Command to cast a column to a different data type.
#[derive(Debug, Clone)]
pub struct CastColumnCommand {
    /// The column name to cast
    pub name: String,
    /// The target type (e.g., "integer", "float", "string")
    pub new_type: String,
    /// Whether to verify existing data can be cast before applying (default: true)
    pub verify_existing: bool,
}

impl CastColumnCommand {
    /// Create a new CastColumnCommand with verify_existing enabled.
    pub fn new(
        name: impl Into<String>,
        new_type: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            new_type: new_type.into(),
            verify_existing: true,
        }
    }
}

impl CommandParsing for CastColumnCommand {
    fn rule() -> Rule {
        Rule::cast_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let raw = pair.as_str().to_uppercase();
        let verify_existing = !raw.contains("NO VERIFY");

        let mut name = None;
        let mut new_type = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    if name.is_none() {
                        name = Some(extract_identifier(&inner));
                    } else {
                        new_type = Some(inner.as_str().to_string());
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "CAST COLUMN statement missing column name".into()
        })?;
        let new_type = new_type.ok_or_else(|| -> BundlebaseError {
            "CAST COLUMN statement missing target type".into()
        })?;

        Ok(CastColumnCommand { name, new_type, verify_existing })
    }

    fn to_statement(&self) -> String {
        format!(
            "CAST COLUMN {} TO {}{}",
            quote_identifier(&self.name),
            self.new_type,
            if !self.verify_existing { " NO VERIFY EXISTING" } else { "" },
        )
    }
}

impl BundleBuilderCommand for CastColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let id = builder.column_id(&self.name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.name)))?;

        let data_type = parse_arrow_type_name(&self.new_type)?;

        // Pre-flight check: scan existing data for non-castable values.
        if self.verify_existing {
            let col_sql = format!("\"{}\"", self.name);
            let check_sql = format!(
                "SELECT CAST({} AS VARCHAR) AS value, COUNT(*) AS cnt \
                 FROM bundle WHERE TRY_CAST({} AS {}) IS NULL AND {} IS NOT NULL \
                 GROUP BY CAST({} AS VARCHAR) ORDER BY cnt DESC LIMIT 10",
                col_sql, col_sql, self.new_type, col_sql, col_sql
            );
            let mut stream = builder.query(&check_sql, vec![], Some(10)).await?;
            let mut found_rows = false;
            let mut samples: Vec<String> = Vec::new();

            while let Some(batch_result) = stream.next().await {
                let batch = batch_result.map_err(|e| BundlebaseError::from(e.to_string()))?;
                if batch.num_rows() > 0 {
                    found_rows = true;
                    let col = batch.column(0);
                    let formatter = arrow::util::display::ArrayFormatter::try_new(
                        col.as_ref(),
                        &Default::default(),
                    )
                    .map_err(|e| BundlebaseError::from(format!("Failed to format values: {}", e)))?;
                    for i in 0..batch.num_rows().min(5) {
                        if !col.is_null(i) {
                            samples.push(formatter.value(i).to_string());
                        }
                    }
                }
            }

            if found_rows {
                let sample_str = samples.join(", ");
                return Err(format!(
                    "CAST COLUMN aborted: non-castable values found in '{}' (e.g. {}). \
                     Use UPDATE or DELETE to clean the data first. \
                     Run 'PROFILE COLUMN \"{}\" FOR CAST TO {}' to see all problem values.",
                    self.name, sample_str, self.name, self.new_type
                ).into());
            }
        }

        builder
            .apply_operation(
                CastColumnOp::setup(id, data_type).into(),
            )
            .await?;

        Ok(format!("Cast column {} to {}", self.name, self.new_type))
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_command;
    use crate::{BundleCommand, CommandParsing};

    #[test]
    fn test_parse_cast_column() {
        let cmd = parse_command("CAST COLUMN price TO Int64").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert!(c.verify_existing);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_no_verify() {
        let cmd = parse_command("CAST COLUMN price TO Int64 NO VERIFY EXISTING").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert!(!c.verify_existing);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_verify_explicit() {
        let cmd = parse_command("CAST COLUMN price TO Int64 VERIFY EXISTING").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert!(c.verify_existing);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_various_types() {
        for type_name in &["Float64", "Utf8", "Boolean", "Date32"] {
            let cmd = parse_command(&format!("CAST COLUMN value TO {}", type_name)).unwrap();
            match cmd {
                BundleCommand::CastColumn(c) => {
                    assert_eq!(c.new_type, *type_name);
                }
                other => panic!("Expected CastColumn, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = super::CastColumnCommand::new("price", "Int64");
        let statement = cmd.to_statement();
        assert_eq!(statement, "CAST COLUMN price TO Int64");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert!(c.verify_existing);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_no_verify() {
        let cmd = super::CastColumnCommand { name: "price".into(), new_type: "Int64".into(), verify_existing: false };
        let statement = cmd.to_statement();
        assert_eq!(statement, "CAST COLUMN price TO Int64 NO VERIFY EXISTING");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert!(!c.verify_existing);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_quoted_column_name() {
        let cmd = parse_command(r#"CAST COLUMN "ResultMeasureValue" TO Float64"#).unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "ResultMeasureValue");
                assert_eq!(c.new_type, "Float64");
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_quoted() {
        let cmd = super::CastColumnCommand::new("column/with.special", "Utf8");
        let statement = cmd.to_statement();
        assert_eq!(statement, r#"CAST COLUMN "column/with.special" TO Utf8"#);

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "column/with.special");
                assert_eq!(c.new_type, "Utf8");
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }
}
