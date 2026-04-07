//! ProfileColumn command implementation (read-only facade).
//!
//! `PROFILE COLUMN <name> [FOR CAST TO <type>]`
//!
//! Without `FOR CAST TO`: shows top values and null counts for the column.
//! With `FOR CAST TO <type>`: shows non-castable values and their counts.

use crate::parser::extract_identifier;
use crate::response::{single_batch_stream, OutputShape};
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use futures::StreamExt;
use std::sync::Arc;

/// Command to profile a column's values.
#[derive(Debug, Clone)]
pub struct ProfileColumnCommand {
    /// The column name to profile
    pub name: String,
    /// If set, show values that cannot be cast to this type
    pub for_cast_to: Option<String>,
}

impl ProfileColumnCommand {
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("value", DataType::Utf8, true),
            Field::new("count", DataType::Int64, false),
        ]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }
}

impl CommandParsing for ProfileColumnCommand {
    fn rule() -> Rule {
        Rule::profile_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut for_cast_to = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                if name.is_none() {
                    name = Some(extract_identifier(&inner));
                } else {
                    for_cast_to = Some(inner.as_str().to_string());
                }
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "PROFILE COLUMN statement missing column name".into()
        })?;

        Ok(ProfileColumnCommand { name, for_cast_to })
    }

    fn to_statement(&self) -> String {
        if let Some(ref cast_type) = self.for_cast_to {
            format!("PROFILE COLUMN \"{}\" FOR CAST TO {}", self.name, cast_type)
        } else {
            format!("PROFILE COLUMN \"{}\"", self.name)
        }
    }
}

impl BundleFacadeCommand for ProfileColumnCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let col_sql = format!("\"{}\"", self.name);
        let output_schema = Self::output_schema();

        let sql = if let Some(ref cast_type) = self.for_cast_to {
            // Show values that cannot be cast to the requested type
            format!(
                "SELECT CAST({} AS VARCHAR) AS value, COUNT(*) AS count \
                 FROM bundle \
                 WHERE TRY_CAST({} AS {}) IS NULL AND {} IS NOT NULL \
                 GROUP BY CAST({} AS VARCHAR) \
                 ORDER BY count DESC \
                 LIMIT 100",
                col_sql, col_sql, cast_type, col_sql, col_sql
            )
        } else {
            // Show top values by frequency
            format!(
                "SELECT CAST({} AS VARCHAR) AS value, COUNT(*) AS count \
                 FROM bundle \
                 WHERE {} IS NOT NULL \
                 GROUP BY CAST({} AS VARCHAR) \
                 ORDER BY count DESC \
                 LIMIT 100",
                col_sql, col_sql, col_sql
            )
        };

        let mut stream = facade.query(&sql, vec![], None).await?;

        let mut values: Vec<Option<String>> = Vec::new();
        let mut counts: Vec<i64> = Vec::new();

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(|e| BundlebaseError::from(e.to_string()))?;
            if batch.num_rows() == 0 {
                continue;
            }
            let val_col = batch.column(0);
            let cnt_col = batch.column(1);
            let cnt_arr = cnt_col
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| BundlebaseError::from("Expected Int64 column for counts"))?;
            let formatter =
                arrow::util::display::ArrayFormatter::try_new(val_col.as_ref(), &Default::default())
                    .map_err(|e| BundlebaseError::from(format!("Failed to format values: {}", e)))?;

            for i in 0..batch.num_rows() {
                values.push(if val_col.is_null(i) {
                    None
                } else {
                    Some(formatter.value(i).to_string())
                });
                counts.push(cnt_arr.value(i));
            }
        }

        if values.is_empty() {
            // Return a single informational row when no results
            let msg = if self.for_cast_to.is_some() {
                "No non-castable values found"
            } else {
                "No non-null values found"
            };
            values.push(Some(msg.to_string()));
            counts.push(0);
        }

        let batch = RecordBatch::try_new(
            Arc::clone(&output_schema),
            vec![
                Arc::new(StringArray::from(values)) as ArrayRef,
                Arc::new(Int64Array::from(counts)) as ArrayRef,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;

        single_batch_stream(output_schema, batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_profile_column_basic() {
        let cmd = parse_command("PROFILE COLUMN value").unwrap();
        match cmd {
            BundleCommand::ProfileColumn(c) => {
                assert_eq!(c.name, "value");
                assert_eq!(c.for_cast_to, None);
            }
            other => panic!("Expected ProfileColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_profile_column_for_cast() {
        let cmd = parse_command("PROFILE COLUMN value FOR CAST TO Float64").unwrap();
        match cmd {
            BundleCommand::ProfileColumn(c) => {
                assert_eq!(c.name, "value");
                assert_eq!(c.for_cast_to, Some("Float64".to_string()));
            }
            other => panic!("Expected ProfileColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_basic() {
        let cmd = ProfileColumnCommand { name: "value".to_string(), for_cast_to: None };
        let stmt = cmd.to_statement();
        assert_eq!(stmt, r#"PROFILE COLUMN "value""#);
        let parsed = parse_command(&stmt).unwrap();
        match parsed {
            BundleCommand::ProfileColumn(c) => {
                assert_eq!(c.name, "value");
                assert_eq!(c.for_cast_to, None);
            }
            other => panic!("Expected ProfileColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_for_cast() {
        let cmd = ProfileColumnCommand { name: "value".to_string(), for_cast_to: Some("Float64".to_string()) };
        let stmt = cmd.to_statement();
        assert_eq!(stmt, r#"PROFILE COLUMN "value" FOR CAST TO Float64"#);
        let parsed = parse_command(&stmt).unwrap();
        match parsed {
            BundleCommand::ProfileColumn(c) => {
                assert_eq!(c.name, "value");
                assert_eq!(c.for_cast_to, Some("Float64".to_string()));
            }
            other => panic!("Expected ProfileColumn, got {:?}", other),
        }
    }
}
