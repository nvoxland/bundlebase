//! DescribeData command implementation (read-only facade).
//!
//! `DESCRIBE DATA IN <col1> [AS <type>], <col2> [AS <type>], ...` returns per-column
//! statistics: min, max, avg, null counts, top 10 most frequent values, and
//! (when AS type is specified) top 10 values that fail to cast to the expected type.

use crate::response::{single_batch_stream, OutputShape};
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use arrow::array::{Array, ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use futures::StreamExt;
use std::sync::Arc;

/// A column specification for DESCRIBE DATA: column name with optional expected type.
#[derive(Debug, Clone)]
pub struct DescribeDataColumnSpec {
    /// Column name (unquoted)
    pub name: String,
    /// Optional expected type for top_10_invalid detection (e.g., "Int64", "Float64")
    pub expected_type: Option<String>,
}

/// Command to analyze data quality and statistics for specified columns.
#[derive(Debug, Clone)]
pub struct DescribeDataCommand {
    pub columns: Vec<DescribeDataColumnSpec>,
}

impl DescribeDataCommand {
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("column", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
            Field::new("min", DataType::Utf8, true),
            Field::new("max", DataType::Utf8, true),
            Field::new("avg", DataType::Utf8, true),
            Field::new("num_nulls", DataType::Int64, false),
            Field::new("num_not_nulls", DataType::Int64, false),
            Field::new("top_10_values", DataType::Utf8, true),
            Field::new("top_10_invalid", DataType::Utf8, true),
            Field::new("distinct_count", DataType::Int64, true),
            Field::new("bloom_filter_present", DataType::Boolean, true),
            Field::new("string_profile", DataType::Utf8, true),
            Field::new("histogram", DataType::Utf8, true),
        ]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }
}

impl CommandParsing for DescribeDataCommand {
    fn rule() -> Rule {
        Rule::describe_data_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut columns = Vec::new();

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::describe_data_column_spec {
                let mut parts = inner.into_inner();
                let name = parts
                    .next()
                    .ok_or_else(|| BundlebaseError::from("DESCRIBE DATA missing column name"))?;
                let real_name = strip_quotes(name.as_str());
                let expected_type = parts.next().map(|p| p.as_str().to_string());

                columns.push(DescribeDataColumnSpec {
                    name: real_name,
                    expected_type,
                });
            }
        }

        if columns.is_empty() {
            return Err("DESCRIBE DATA IN requires at least one column".into());
        }

        Ok(DescribeDataCommand { columns })
    }

    fn to_statement(&self) -> String {
        let cols: Vec<String> = self
            .columns
            .iter()
            .map(|c| {
                if let Some(ref t) = c.expected_type {
                    format!("\"{}\" AS {}", c.name, t)
                } else {
                    format!("\"{}\"", c.name)
                }
            })
            .collect();
        format!("DESCRIBE DATA IN {}", cols.join(", "))
    }
}

/// Strip surrounding double quotes from an identifier, if present.
fn strip_quotes(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Returns true if the Arrow DataType is numeric (supports MIN/MAX/AVG).
fn is_numeric(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal128(_, _)
            | DataType::Decimal256(_, _)
    )
}

/// Returns true if the Arrow DataType is date/timestamp (supports MIN/MAX but not AVG).
fn is_temporal(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::Date32
            | DataType::Date64
            | DataType::Timestamp(_, _)
            | DataType::Time32(_)
            | DataType::Time64(_)
    )
}

/// Extract a string value from column `col_index` at row 0 of the first batch.
/// Handles any Arrow string type (Utf8, LargeUtf8) by using display formatting.
fn extract_string_from_batch(batch: &RecordBatch, col_index: usize) -> Option<String> {
    if batch.num_rows() == 0 {
        return None;
    }
    let col = batch.column(col_index);
    if col.is_null(0) {
        return None;
    }
    // Use Arrow's display formatting which works for any type
    let formatter = arrow::util::display::ArrayFormatter::try_new(col.as_ref(), &Default::default());
    match formatter {
        Ok(f) => Some(f.value(0).to_string()),
        Err(_) => None,
    }
}

/// Extract an i64 value from column `col_index` at row 0 of the first batch.
fn extract_i64_from_batch(batch: &RecordBatch, col_index: usize) -> i64 {
    if batch.num_rows() == 0 {
        return 0;
    }
    let col = batch.column(col_index);
    if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        arr.value(0)
    } else {
        0
    }
}

/// Build a JSON array string of [{value, count}, ...] from a two-column query result.
async fn extract_value_counts(
    facade: &dyn BundleFacade,
    sql: &str,
) -> Result<Option<String>, BundlebaseError> {
    let mut stream = facade.query(sql, vec![], None).await?;
    let mut entries: Vec<serde_json::Value> = Vec::new();

    while let Some(batch_result) = stream.next().await {
        let batch = batch_result.map_err(|e| BundlebaseError::from(e.to_string()))?;
        if batch.num_rows() == 0 {
            continue;
        }
        let values_col = batch.column(0);
        let counts_col = batch.column(1);
        let counts = counts_col
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| BundlebaseError::from("Expected Int64 column for counts"))?;

        let formatter = arrow::util::display::ArrayFormatter::try_new(
            values_col.as_ref(),
            &Default::default(),
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to format values: {}", e)))?;

        for i in 0..batch.num_rows() {
            if !values_col.is_null(i) {
                entries.push(serde_json::json!({
                    "value": formatter.value(i).to_string(),
                    "count": counts.value(i),
                }));
            }
        }
    }

    if entries.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(&entries)
            .map(Some)
            .map_err(|e| BundlebaseError::from(format!("Failed to serialize value counts: {}", e)))
    }
}

/// Load pre-computed column stats from all blocks in the bundle, aggregated by column name.
///
/// For each requested column, finds its ColumnId, then gathers stats from all blocks.
/// Aggregates: max distinct_count, first non-None string_profile, first non-empty histogram.
async fn load_column_stats(
    facade: &dyn BundleFacade,
    column_names: &[&str],
) -> std::collections::HashMap<String, bundlebase_data::ColumnStats> {
    let mut result = std::collections::HashMap::new();

    // Resolve column names to IDs once before iterating blocks.
    let col_id_pairs: Vec<(&str, _)> = column_names.iter()
        .filter_map(|&n| facade.column_id(n).map(|id| (n, id)))
        .collect();

    if col_id_pairs.is_empty() {
        return result;
    }

    let packs = facade.packs();
    for (_, pack) in &packs {
        for block in pack.blocks() {
            let all_stats = match block.reader().column_stats().await {
                Ok(s) if !s.is_empty() => s,
                _ => continue,
            };
            let col_ids = block.column_ids();

            for &(col_name, col_id) in &col_id_pairs {
                let block_col_idx = match col_ids.iter().position(|id| *id == col_id) {
                    Some(i) => i,
                    None => continue,
                };
                let stat = match all_stats.get(block_col_idx) {
                    Some(s) => s.clone(),
                    None => continue,
                };

                let entry = result.entry(col_name.to_string()).or_insert_with(|| stat.clone());
                // Aggregate: prefer higher distinct_count, first non-None profiles
                if stat.distinct_count > entry.distinct_count {
                    entry.distinct_count = stat.distinct_count;
                }
                if entry.string_profile.is_none() {
                    entry.string_profile = stat.string_profile;
                }
                if entry.histogram.is_empty() {
                    entry.histogram = stat.histogram;
                }
                // Merge page_stats for bloom filter presence check
                if entry.page_stats.is_empty() {
                    entry.page_stats = stat.page_stats;
                }
            }
        }
    }

    result
}

impl BundleFacadeCommand for DescribeDataCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let schema = facade.schema().await?;

        // Validate all columns exist and collect their types
        struct ColInfo {
            name: String,
            data_type: DataType,
            expected_type: Option<String>,
        }
        let mut col_infos: Vec<ColInfo> = Vec::new();
        for spec in &self.columns {
            let field = schema
                .field_with_name(&spec.name)
                .map_err(|_| BundlebaseError::from(format!("Column '{}' not found", spec.name)))?;
            col_infos.push(ColInfo {
                name: spec.name.clone(),
                data_type: field.data_type().clone(),
                expected_type: spec.expected_type.clone(),
            });
        }

        // Build a single SQL query for all basic stats (min, max, avg, null counts)
        // This scans the data once instead of N times per column.
        let mut select_parts: Vec<String> = Vec::new();
        for (i, info) in col_infos.iter().enumerate() {
            let q = format!("\"{}\"", info.name);
            if is_numeric(&info.data_type) {
                select_parts.push(format!("CAST(MIN({}) AS VARCHAR) AS min_{}", q, i));
                select_parts.push(format!("CAST(MAX({}) AS VARCHAR) AS max_{}", q, i));
                select_parts.push(format!("CAST(AVG(CAST({} AS DOUBLE)) AS VARCHAR) AS avg_{}", q, i));
            } else if is_temporal(&info.data_type) {
                select_parts.push(format!("CAST(MIN({}) AS VARCHAR) AS min_{}", q, i));
                select_parts.push(format!("CAST(MAX({}) AS VARCHAR) AS max_{}", q, i));
                select_parts.push(format!("CAST(NULL AS VARCHAR) AS avg_{}", i));
            } else {
                select_parts.push(format!("CAST(NULL AS VARCHAR) AS min_{}", i));
                select_parts.push(format!("CAST(NULL AS VARCHAR) AS max_{}", i));
                select_parts.push(format!("CAST(NULL AS VARCHAR) AS avg_{}", i));
            }
            select_parts.push(format!("COUNT(*) - COUNT({}) AS nulls_{}", q, i));
            select_parts.push(format!("COUNT({}) AS notnulls_{}", q, i));
        }

        let stats_sql = format!("SELECT {} FROM bundle", select_parts.join(", "));
        let mut stats_stream = facade.query(&stats_sql, vec![], None).await?;
        let stats_batch = stats_stream
            .next()
            .await
            .ok_or_else(|| BundlebaseError::from("No results from stats query"))?
            .map_err(|e| BundlebaseError::from(e.to_string()))?;

        // Extract results from the single stats batch
        // Load pre-computed column stats (available for CSV/JSONL blocks with layout files).
        // For each requested column, aggregate across all blocks: max distinct count,
        // first non-None string_profile, first non-empty histogram, bloom present in any page.
        let col_stats_map = load_column_stats(facade, &col_infos.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()).await;

        let mut real_names: Vec<String> = Vec::new();
        let mut col_data_types: Vec<String> = Vec::new();
        let mut mins: Vec<Option<String>> = Vec::new();
        let mut maxs: Vec<Option<String>> = Vec::new();
        let mut avgs: Vec<Option<String>> = Vec::new();
        let mut null_counts: Vec<i64> = Vec::new();
        let mut not_null_counts: Vec<i64> = Vec::new();
        let mut top_10_values_list: Vec<Option<String>> = Vec::new();
        let mut top_10_invalid_list: Vec<Option<String>> = Vec::new();
        let mut distinct_counts: Vec<Option<i64>> = Vec::new();
        let mut bloom_filter_presents: Vec<Option<bool>> = Vec::new();
        let mut string_profiles: Vec<Option<String>> = Vec::new();
        let mut histograms: Vec<Option<String>> = Vec::new();

        for (i, info) in col_infos.iter().enumerate() {
            let base_idx = i * 5; // 5 columns per analyzed column (min, max, avg, nulls, not_nulls)

            real_names.push(info.name.clone());
            col_data_types.push(format!("{}", info.data_type));

            mins.push(extract_string_from_batch(&stats_batch, base_idx));
            maxs.push(extract_string_from_batch(&stats_batch, base_idx + 1));
            avgs.push(extract_string_from_batch(&stats_batch, base_idx + 2));
            null_counts.push(extract_i64_from_batch(&stats_batch, base_idx + 3));
            not_null_counts.push(extract_i64_from_batch(&stats_batch, base_idx + 4));

            // Top 10 values - requires a separate GROUP BY query per column
            let quoted_col = format!("\"{}\"", info.name);
            let top_values_sql = format!(
                "SELECT CAST({} AS VARCHAR) AS value, COUNT(*) AS cnt \
                 FROM bundle WHERE {} IS NOT NULL \
                 GROUP BY CAST({} AS VARCHAR) ORDER BY cnt DESC LIMIT 10",
                quoted_col, quoted_col, quoted_col
            );
            top_10_values_list.push(extract_value_counts(facade, &top_values_sql).await?);

            // Top 10 invalid (only when AS type specified)
            if let Some(ref expected) = info.expected_type {
                let invalid_sql = format!(
                    "SELECT CAST({} AS VARCHAR) AS value, COUNT(*) AS cnt \
                     FROM bundle WHERE {} IS NOT NULL \
                     AND TRY_CAST({} AS {}) IS NULL \
                     GROUP BY CAST({} AS VARCHAR) ORDER BY cnt DESC LIMIT 10",
                    quoted_col, quoted_col, quoted_col, expected, quoted_col
                );
                top_10_invalid_list.push(extract_value_counts(facade, &invalid_sql).await?);
            } else {
                top_10_invalid_list.push(None);
            }

            // Pre-computed stats from layout files (CSV/JSONL) or Parquet attach-time layout.
            if let Some(col_stat) = col_stats_map.get(info.name.as_str()) {
                distinct_counts.push(if col_stat.distinct_count > 0 {
                    Some(col_stat.distinct_count as i64)
                } else {
                    None
                });
                let has_bloom = col_stat.page_stats.iter().any(|p| p.bloom_filter.is_some());
                bloom_filter_presents.push(Some(has_bloom));
                string_profiles.push(col_stat.string_profile.as_ref().and_then(|sp| {
                    serde_json::to_string(&serde_json::json!({
                        "min_len": sp.min_len,
                        "max_len": sp.max_len,
                        "avg_len": sp.avg_len,
                        "pct_ascii": sp.pct_ascii,
                    })).ok()
                }));
                histograms.push(if col_stat.histogram.is_empty() {
                    None
                } else {
                    serde_json::to_string(&col_stat.histogram.iter().map(|b| serde_json::json!({
                        "lower_bound": b.lower_bound.display(),
                        "count": b.count,
                    })).collect::<Vec<_>>()).ok()
                });
            } else {
                distinct_counts.push(None);
                bloom_filter_presents.push(None);
                string_profiles.push(None);
                histograms.push(None);
            }
        }

        let output_schema = Self::output_schema();

        let batch = RecordBatch::try_new(
            Arc::clone(&output_schema),
            vec![
                Arc::new(StringArray::from(real_names)) as ArrayRef,
                Arc::new(StringArray::from(col_data_types)) as ArrayRef,
                Arc::new(StringArray::from(mins)) as ArrayRef,
                Arc::new(StringArray::from(maxs)) as ArrayRef,
                Arc::new(StringArray::from(avgs)) as ArrayRef,
                Arc::new(Int64Array::from(null_counts)) as ArrayRef,
                Arc::new(Int64Array::from(not_null_counts)) as ArrayRef,
                Arc::new(StringArray::from(top_10_values_list)) as ArrayRef,
                Arc::new(StringArray::from(top_10_invalid_list)) as ArrayRef,
                Arc::new(Int64Array::from(distinct_counts)) as ArrayRef,
                Arc::new(BooleanArray::from(bloom_filter_presents)) as ArrayRef,
                Arc::new(StringArray::from(string_profiles)) as ArrayRef,
                Arc::new(StringArray::from(histograms)) as ArrayRef,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;

        single_batch_stream(output_schema, batch)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_describe_data_basic() {
        let cmd = parse_command("DESCRIBE DATA IN age, salary")
            .expect("Failed to parse DESCRIBE DATA IN");
        match cmd {
            BundleCommand::DescribeData(c) => {
                assert_eq!(c.columns.len(), 2);
                assert_eq!(c.columns[0].name, "age");
                assert_eq!(c.columns[0].expected_type, None);
                assert_eq!(c.columns[1].name, "salary");
                assert_eq!(c.columns[1].expected_type, None);
            }
            _ => panic!("Expected DescribeData variant"),
        }
    }

    #[test]
    fn test_parse_describe_data_with_as_type() {
        let cmd = parse_command("DESCRIBE DATA IN price AS Float64, name")
            .expect("Failed to parse DESCRIBE DATA IN with AS");
        match cmd {
            BundleCommand::DescribeData(c) => {
                assert_eq!(c.columns.len(), 2);
                assert_eq!(c.columns[0].name, "price");
                assert_eq!(c.columns[0].expected_type, Some("Float64".to_string()));
                assert_eq!(c.columns[1].name, "name");
                assert_eq!(c.columns[1].expected_type, None);
            }
            _ => panic!("Expected DescribeData variant"),
        }
    }

    #[test]
    fn test_parse_describe_data_case_insensitive() {
        let cmd = parse_command("describe data in age")
            .expect("Failed to parse describe data in (lowercase)");
        match cmd {
            BundleCommand::DescribeData(c) => {
                assert_eq!(c.columns.len(), 1);
                assert_eq!(c.columns[0].name, "age");
            }
            _ => panic!("Expected DescribeData variant"),
        }
    }

    #[test]
    fn test_parse_describe_data_quoted_identifiers() {
        let cmd = parse_command("DESCRIBE DATA IN \"First Name\", \"Last Name\"")
            .expect("Failed to parse DESCRIBE DATA IN with quoted identifiers");
        match cmd {
            BundleCommand::DescribeData(c) => {
                assert_eq!(c.columns.len(), 2);
                assert_eq!(c.columns[0].name, "First Name");
                assert_eq!(c.columns[1].name, "Last Name");
            }
            _ => panic!("Expected DescribeData variant"),
        }
    }

    #[test]
    fn test_parse_describe_data_single_column() {
        let cmd = parse_command("DESCRIBE DATA IN id")
            .expect("Failed to parse single column DESCRIBE DATA");
        match cmd {
            BundleCommand::DescribeData(c) => {
                assert_eq!(c.columns.len(), 1);
                assert_eq!(c.columns[0].name, "id");
            }
            _ => panic!("Expected DescribeData variant"),
        }
    }

    #[test]
    fn test_parse_describe_data_roundtrip() {
        let cmd = DescribeDataCommand {
            columns: vec![
                DescribeDataColumnSpec {
                    name: "age".to_string(),
                    expected_type: None,
                },
                DescribeDataColumnSpec {
                    name: "price".to_string(),
                    expected_type: Some("Float64".to_string()),
                },
            ],
        };
        let statement = cmd.to_statement();
        assert_eq!(statement, "DESCRIBE DATA IN \"age\", \"price\" AS Float64");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::DescribeData(c) => {
                assert_eq!(c.columns.len(), 2);
                assert_eq!(c.columns[0].name, "age");
                assert_eq!(c.columns[0].expected_type, None);
                assert_eq!(c.columns[1].name, "price");
                assert_eq!(c.columns[1].expected_type, Some("Float64".to_string()));
            }
            _ => panic!("Expected DescribeData variant"),
        }
    }
}
