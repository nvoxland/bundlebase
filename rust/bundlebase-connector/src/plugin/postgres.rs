//! PostgreSQL connector.
//!
//! Extracts data from a PostgreSQL database query, partitioning
//! results into fixed-size chunks based on a sort column.
//!
//! Arguments:
//! - `url` (required): PostgreSQL connection URL (postgres://user:pass@host:port/dbname)
//! - `query` (required): SQL query to execute
//! - `sort_column` (required): Column to ORDER BY and partition on
//! - `batch_size` (optional): Rows per output file (default: 1000000)

use arrow::array::{
    ArrayRef, BooleanBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, StringBuilder, TimestampMicrosecondBuilder,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use bundlebase_common::connector::{
    ArgSpec, Connector, ConnectorSignature, DiscoveredLocation, SourceData, SourceFormat,
};
use bundlebase_common::source_utils as shared_utils;
use bundlebase_common::{BundlebaseError, ConfigProvider};
use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_postgres::{Client, NoTls, Row};
use url::Url;

/// Number of rows fetched per page during streaming data retrieval.
const PAGE_SIZE: usize = 10_000;

/// Dollar-quote tag for safe SQL value interpolation.
const DOLLAR_TAG: &str = "$__bb$";

/// PostgreSQL connector.
///
/// Extracts data from a PostgreSQL query, partitioned into chunks by row count.
pub struct PostgresConnector;

/// JSON-serializable location identifying a partition by sort column range.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PartitionLocation {
    sort_col: String,
    min: String,
    max: String,
}

/// A partition boundary discovered during the discover phase.
#[derive(Debug, Clone)]
struct PartitionBoundary {
    /// Location string: JSON `{"sort_col":"id","min":"1","max":"100"}`
    location: String,
    /// Version string: "count:checksum"
    version: String,
}

impl PostgresConnector {
    /// Connect to PostgreSQL database.
    async fn connect(url: &str) -> Result<Client, BundlebaseError> {
        let (client, connection) = tokio_postgres::connect(url, NoTls).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to connect to PostgreSQL: {}", e))
        })?;

        // Spawn the connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("PostgreSQL connection error: {}", e);
            }
        });

        Ok(client)
    }

    /// Validate that a SQL identifier contains only safe characters.
    ///
    /// Accepts alphanumeric, underscore, and dot (for schema-qualified names).
    fn validate_identifier(name: &str) -> Result<(), BundlebaseError> {
        if name.is_empty() {
            return Err("Identifier cannot be empty".into());
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            return Err(format!(
                "Invalid identifier '{}': only alphanumeric, underscore, and dot characters are allowed",
                name
            )
            .into());
        }
        Ok(())
    }

    /// Dollar-quote a value for safe SQL interpolation.
    fn dollar_quote(value: &str) -> String {
        format!("{}{}{}", DOLLAR_TAG, value, DOLLAR_TAG)
    }

    /// Parse a location JSON string into a `PartitionLocation`.
    fn parse_location(location: &str) -> Result<PartitionLocation, BundlebaseError> {
        serde_json::from_str(location).map_err(|e| {
            BundlebaseError::from(format!("Invalid location JSON: {}: {}", location, e))
        })
    }

    /// Build a location JSON string from components.
    fn build_location(sort_column: &str, min_val: &str, max_val: &str) -> String {
        serde_json::to_string(&PartitionLocation {
            sort_col: sort_column.to_string(),
            min: min_val.to_string(),
            max: max_val.to_string(),
        })
        .expect("PartitionLocation serialization cannot fail")
    }

    /// Parse attached_locations to extract existing partition boundaries.
    ///
    /// Returns `Vec<(min, max)>` pairs for locations matching the given sort_column.
    fn parse_attached_boundaries(
        attached_locations: &HashSet<String>,
        sort_column: &str,
    ) -> Vec<(String, String)> {
        let mut boundaries = Vec::new();
        for loc in attached_locations {
            if let Ok(parsed) = Self::parse_location(loc) {
                if parsed.sort_col == sort_column {
                    boundaries.push((parsed.min, parsed.max));
                }
            }
        }
        // Sort by min value for consistent ordering
        boundaries.sort_by(|a, b| a.0.cmp(&b.0));
        boundaries
    }

    /// Discover partition boundaries.
    ///
    /// When `existing_boundaries` is empty (initial fetch), partitions the entire
    /// dataset using a window function. When non-empty (incremental sync),
    /// re-verifies existing partitions and discovers new tail data.
    async fn discover_partitions(
        client: &Client,
        query: &str,
        sort_column: &str,
        batch_size: usize,
        existing_boundaries: &[(String, String)],
    ) -> Result<Vec<PartitionBoundary>, BundlebaseError> {
        let base_query = query.trim_end_matches(';');
        let mut partitions = Vec::new();

        // Re-verify existing partitions (skip when initial)
        if !existing_boundaries.is_empty() {
            let mut case_arms = String::new();
            for (idx, (min_val, max_val)) in existing_boundaries.iter().enumerate() {
                case_arms.push_str(&format!(
                    "WHEN {sort_col}::text >= {min} AND {sort_col}::text <= {max} THEN {idx} ",
                    sort_col = sort_column,
                    min = Self::dollar_quote(min_val),
                    max = Self::dollar_quote(max_val),
                    idx = idx
                ));
            }

            let sql = format!(
                "SELECT \
                   CASE {cases} ELSE -1 END AS part_idx, \
                   count(*) AS cnt, \
                   sum(hashtext(row_to_json(_q.*)::text)::bigint) AS chk, \
                   min({sort_col}::text) AS min_val, \
                   max({sort_col}::text) AS max_val \
                 FROM ({base}) AS _q \
                 GROUP BY part_idx ORDER BY part_idx",
                cases = case_arms,
                sort_col = sort_column,
                base = base_query
            );

            let rows = client
                .query(&sql, &[])
                .await
                .map_err(|e| BundlebaseError::from(format!("Discovery query failed: {}", e)))?;

            let mut has_tail = false;
            for row in &rows {
                let part_idx: i32 = row.get("part_idx");
                let count: i64 = row.get("cnt");
                let checksum: i64 = row.get("chk");

                if part_idx >= 0 {
                    let idx = part_idx as usize;
                    let (orig_min, orig_max) = &existing_boundaries[idx];
                    partitions.push(PartitionBoundary {
                        location: Self::build_location(sort_column, orig_min, orig_max),
                        version: format!("{}:{}", count, checksum),
                    });
                } else {
                    has_tail = true;
                }
            }

            if !has_tail {
                return Ok(partitions);
            }
        }

        // Partition new data using window function.
        // For initial discovery this covers all rows; for incremental it covers
        // only rows outside existing boundaries.
        let where_clause = if existing_boundaries.is_empty() {
            String::new()
        } else {
            let exclusions: Vec<String> = existing_boundaries
                .iter()
                .map(|(min_val, max_val)| {
                    format!(
                        "NOT ({sort_col}::text >= {min} AND {sort_col}::text <= {max})",
                        sort_col = sort_column,
                        min = Self::dollar_quote(min_val),
                        max = Self::dollar_quote(max_val)
                    )
                })
                .collect();
            format!("WHERE {}", exclusions.join(" AND "))
        };

        let sql = format!(
            "SELECT pnum, min(sort_val) AS min_val, max(sort_val) AS max_val, \
             count(*) AS cnt, sum(hashtext(row_json)::bigint) AS chk \
             FROM ( \
               SELECT {sort_col}::text AS sort_val, \
                      row_to_json(_q.*)::text AS row_json, \
                      floor((row_number() OVER (ORDER BY {sort_col}) - 1) / {batch}) AS pnum \
               FROM ({base}) AS _q \
               {where_clause} \
             ) AS _p \
             GROUP BY pnum ORDER BY pnum",
            sort_col = sort_column,
            batch = batch_size,
            base = base_query,
            where_clause = where_clause
        );

        let rows = client
            .query(&sql, &[])
            .await
            .map_err(|e| BundlebaseError::from(format!("Discovery query failed: {}", e)))?;

        for row in &rows {
            let min_val: String = row.get("min_val");
            let max_val: String = row.get("max_val");
            let count: i64 = row.get("cnt");
            let checksum: i64 = row.get("chk");

            partitions.push(PartitionBoundary {
                location: Self::build_location(sort_column, &min_val, &max_val),
                version: format!("{}:{}", count, checksum),
            });
        }

        Ok(partitions)
    }

    /// Stream data for a range using keyset pagination.
    ///
    /// Returns a BoxStream of RecordBatches, each containing up to PAGE_SIZE rows.
    fn stream_range_as_batches(
        client: Arc<Client>,
        query: String,
        sort_column: String,
        min_val: String,
        max_val: String,
    ) -> BoxStream<'static, Result<RecordBatch, BundlebaseError>> {
        struct PageState {
            client: Arc<Client>,
            query: String,
            sort_column: String,
            current_min: String,
            max_val: String,
            is_first_page: bool,
            done: bool,
        }

        let state = PageState {
            client,
            query,
            sort_column,
            current_min: min_val,
            max_val,
            is_first_page: true,
            done: false,
        };

        futures::stream::unfold(state, |mut state| async move {
            if state.done {
                return None;
            }

            let base_query = state.query.trim_end_matches(';');

            // First page: >= min_val; subsequent pages: > current_min
            let compare_op = if state.is_first_page { ">=" } else { ">" };

            let page_query = format!(
                "SELECT * FROM ({base}) AS _q \
                 WHERE {col} {op} {min} AND {col} <= {max} \
                 ORDER BY {col} ASC LIMIT {limit}",
                base = base_query,
                col = state.sort_column,
                op = compare_op,
                min = Self::dollar_quote(&state.current_min),
                max = Self::dollar_quote(&state.max_val),
                limit = PAGE_SIZE
            );

            let rows = match state.client.query(&page_query, &[]).await {
                Ok(rows) => rows,
                Err(e) => {
                    state.done = true;
                    return Some((
                        Err(BundlebaseError::from(format!("Page query failed: {}", e))),
                        state,
                    ));
                }
            };

            if rows.is_empty() {
                return None;
            }

            let row_count = rows.len();

            // Find sort column index to get the last value for keyset pagination
            let sort_col_idx = rows[0]
                .columns()
                .iter()
                .position(|c| c.name() == state.sort_column);

            if let Some(idx) = sort_col_idx {
                match Self::get_value_as_string(&rows[rows.len() - 1], idx) {
                    Ok(last_val) => {
                        state.current_min = last_val;
                    }
                    Err(e) => {
                        state.done = true;
                        return Some((Err(e), state));
                    }
                }
            }

            state.is_first_page = false;

            if row_count < PAGE_SIZE {
                state.done = true;
            }

            match Self::rows_to_record_batch(&rows) {
                Ok(batch) => Some((Ok(batch), state)),
                Err(e) => {
                    state.done = true;
                    Some((Err(e), state))
                }
            }
        })
        .boxed()
    }

    /// Get a column value as a string for source_location.
    fn get_value_as_string(row: &Row, col_idx: usize) -> Result<String, BundlebaseError> {
        let columns = row.columns();
        let col_type = columns[col_idx].type_();

        // Handle different PostgreSQL types
        Ok(match col_type.name() {
            "int2" => row
                .get::<_, Option<i16>>(col_idx)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            "int4" => row
                .get::<_, Option<i32>>(col_idx)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            "int8" => row
                .get::<_, Option<i64>>(col_idx)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            "float4" => row
                .get::<_, Option<f32>>(col_idx)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            "float8" => row
                .get::<_, Option<f64>>(col_idx)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            "bool" => row
                .get::<_, Option<bool>>(col_idx)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string()),
            "timestamp" | "timestamptz" => row
                .get::<_, Option<chrono::NaiveDateTime>>(col_idx)
                .map(|v| v.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_else(|| "null".to_string()),
            _ => row
                .get::<_, Option<String>>(col_idx)
                .unwrap_or_else(|| "null".to_string()),
        })
    }

    /// Convert PostgreSQL rows to Arrow RecordBatch.
    fn rows_to_record_batch(rows: &[Row]) -> Result<RecordBatch, BundlebaseError> {
        if rows.is_empty() {
            return Err("Cannot create RecordBatch from empty rows".into());
        }

        let columns = rows[0].columns();
        let mut fields = Vec::new();
        let mut arrays: Vec<ArrayRef> = Vec::new();

        for (col_idx, col) in columns.iter().enumerate() {
            let col_name = col.name();
            let col_type = col.type_();

            let (field, array): (Field, ArrayRef) = match col_type.name() {
                "int2" => {
                    let mut builder = Int16Builder::new();
                    for row in rows {
                        builder.append_option(row.get::<_, Option<i16>>(col_idx));
                    }
                    (
                        Field::new(col_name, DataType::Int16, true),
                        Arc::new(builder.finish()),
                    )
                }
                "int4" => {
                    let mut builder = Int32Builder::new();
                    for row in rows {
                        builder.append_option(row.get::<_, Option<i32>>(col_idx));
                    }
                    (
                        Field::new(col_name, DataType::Int32, true),
                        Arc::new(builder.finish()),
                    )
                }
                "int8" => {
                    let mut builder = Int64Builder::new();
                    for row in rows {
                        builder.append_option(row.get::<_, Option<i64>>(col_idx));
                    }
                    (
                        Field::new(col_name, DataType::Int64, true),
                        Arc::new(builder.finish()),
                    )
                }
                "float4" => {
                    let mut builder = Float32Builder::new();
                    for row in rows {
                        builder.append_option(row.get::<_, Option<f32>>(col_idx));
                    }
                    (
                        Field::new(col_name, DataType::Float32, true),
                        Arc::new(builder.finish()),
                    )
                }
                "float8" => {
                    let mut builder = Float64Builder::new();
                    for row in rows {
                        builder.append_option(row.get::<_, Option<f64>>(col_idx));
                    }
                    (
                        Field::new(col_name, DataType::Float64, true),
                        Arc::new(builder.finish()),
                    )
                }
                "bool" => {
                    let mut builder = BooleanBuilder::new();
                    for row in rows {
                        builder.append_option(row.get::<_, Option<bool>>(col_idx));
                    }
                    (
                        Field::new(col_name, DataType::Boolean, true),
                        Arc::new(builder.finish()),
                    )
                }
                "timestamp" | "timestamptz" => {
                    let mut builder = TimestampMicrosecondBuilder::new();
                    for row in rows {
                        let ts: Option<chrono::NaiveDateTime> = row.get(col_idx);
                        builder.append_option(ts.map(|t| t.and_utc().timestamp_micros()));
                    }
                    (
                        Field::new(
                            col_name,
                            DataType::Timestamp(TimeUnit::Microsecond, None),
                            true,
                        ),
                        Arc::new(builder.finish()),
                    )
                }
                _ => {
                    // Default to string for unknown types
                    let mut builder = StringBuilder::new();
                    for row in rows {
                        let val: Option<String> = row.get(col_idx);
                        builder.append_option(val);
                    }
                    (
                        Field::new(col_name, DataType::Utf8, true),
                        Arc::new(builder.finish()),
                    )
                }
            };

            fields.push(field);
            arrays.push(array);
        }

        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, arrays)
            .map_err(|e| BundlebaseError::from(format!("Failed to create RecordBatch: {}", e)))
    }

    /// Parse batch_size from args with default.
    fn get_batch_size(args: &HashMap<String, String>) -> Result<usize, BundlebaseError> {
        match args.get("batch_size") {
            Some(s) => s
                .parse::<usize>()
                .map_err(|_| BundlebaseError::from("batch_size must be a positive integer")),
            None => Ok(1_000_000),
        }
    }
}

#[async_trait]
impl Connector for PostgresConnector {
    fn signature(&self) -> ConnectorSignature {
        ConnectorSignature {
            name: "postgres".to_string(),
            arg_specs: vec![
                ArgSpec {
                    name: "url",
                    description:
                        "PostgreSQL connection URL (postgres://user:pass@host:port/dbname)",
                    required: true,
                    default: None,
                },
                ArgSpec {
                    name: "query",
                    description: "SQL query to execute",
                    required: true,
                    default: None,
                },
                ArgSpec {
                    name: "sort_column",
                    description: "Column to sort and partition by (e.g., id, created_at)",
                    required: true,
                    default: None,
                },
                ArgSpec {
                    name: "batch_size",
                    description: "Number of rows per output file",
                    required: false,
                    default: Some("1000000"),
                },
            ],
            accepts_extra_args: false,
        }
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // Validate URL format
        let url = shared_utils::require_arg(args, "url", "postgres")?;
        if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
            return Err("url must be a PostgreSQL connection URL (postgres://...)".into());
        }

        // Validate sort_column is a safe identifier
        if let Some(sort_column) = args.get("sort_column") {
            Self::validate_identifier(sort_column)?;
        }

        // Validate batch_size if provided
        if let Some(batch_size) = args.get("batch_size") {
            batch_size
                .parse::<usize>()
                .map_err(|_| BundlebaseError::from("batch_size must be a positive integer"))?;
        }

        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let url = shared_utils::require_arg(args, "url", "postgres")?;
        let query = shared_utils::require_arg(args, "query", "postgres")?;
        let sort_column = shared_utils::require_arg(args, "sort_column", "postgres")?;
        let batch_size = Self::get_batch_size(args)?;

        Self::validate_identifier(sort_column)?;

        let client = Self::connect(url).await?;

        let existing = Self::parse_attached_boundaries(attached_locations, sort_column);

        let partitions =
            Self::discover_partitions(&client, query, sort_column, batch_size, &existing).await?;

        Ok(partitions
            .into_iter()
            .map(|p| DiscoveredLocation {
                location: p.location,
                must_copy: true,
                format: SourceFormat::Parquet,
                version: p.version,
            })
            .collect())
    }

    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        let url = shared_utils::require_arg(args, "url", "postgres")?;
        let query = shared_utils::require_arg(args, "query", "postgres")?;
        let sort_column = shared_utils::require_arg(args, "sort_column", "postgres")?;

        let parsed = Self::parse_location(&location.location)?;

        let client = Arc::new(Self::connect(url).await?);
        let stream = Self::stream_range_as_batches(
            client,
            query.to_string(),
            sort_column.to_string(),
            parsed.min,
            parsed.max,
        );

        Ok(Some(SourceData::Arrow(stream)))
    }

    async fn stable_url(
        &self,
        _location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<Url>, BundlebaseError> {
        // Postgres data is generated from queries, no stable URL
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase_common::connector::validate_connector_args;

    #[test]
    fn test_signature() {
        let func = PostgresConnector;
        let sig = func.signature();
        assert_eq!(sig.name, "postgres");
        assert_eq!(sig.arg_specs.len(), 4);
        assert!(sig.arg_specs.iter().any(|s| s.name == "url" && s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "query" && s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "sort_column" && s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "batch_size" && !s.required));
        // Check default batch_size is 1000000
        let batch_spec = sig
            .arg_specs
            .iter()
            .find(|s| s.name == "batch_size")
            .expect("batch_size spec");
        assert_eq!(batch_spec.default, Some("1000000"));
    }

    #[test]
    fn test_validate_args_with_valid_url() {
        let func = PostgresConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "postgres://user:pass@localhost/db".to_string(),
        );
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        assert!({
            let sig = func.signature();
            validate_connector_args(&args, &sig).and_then(|_| func.validate_args(&args))
        }
        .is_ok());
    }

    #[test]
    fn test_validate_args_with_postgresql_url() {
        let func = PostgresConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "postgresql://user:pass@localhost/db".to_string(),
        );
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        assert!({
            let sig = func.signature();
            validate_connector_args(&args, &sig).and_then(|_| func.validate_args(&args))
        }
        .is_ok());
    }

    #[test]
    fn test_validate_args_missing_url() {
        let func = PostgresConnector;
        let mut args = HashMap::new();
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());

        let result = {
            let sig = func.signature();
            validate_connector_args(&args, &sig).and_then(|_| func.validate_args(&args))
        };
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_args_invalid_url() {
        let func = PostgresConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "mysql://user:pass@localhost/db".to_string(),
        );
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());

        let result = {
            let sig = func.signature();
            validate_connector_args(&args, &sig).and_then(|_| func.validate_args(&args))
        };
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("PostgreSQL"));
    }

    #[test]
    fn test_validate_args_invalid_batch_size() {
        let func = PostgresConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "postgres://user:pass@localhost/db".to_string(),
        );
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        args.insert("batch_size".to_string(), "not_a_number".to_string());

        let result = {
            let sig = func.signature();
            validate_connector_args(&args, &sig).and_then(|_| func.validate_args(&args))
        };
        assert!(result.is_err());
    }

    // --- validate_identifier tests ---

    #[test]
    fn test_validate_identifier_simple() {
        assert!(PostgresConnector::validate_identifier("id").is_ok());
        assert!(PostgresConnector::validate_identifier("created_at").is_ok());
        assert!(PostgresConnector::validate_identifier("public.users").is_ok());
        assert!(PostgresConnector::validate_identifier("col123").is_ok());
    }

    #[test]
    fn test_validate_identifier_rejects_special_chars() {
        assert!(PostgresConnector::validate_identifier("id; DROP TABLE").is_err());
        assert!(PostgresConnector::validate_identifier("col'name").is_err());
        assert!(PostgresConnector::validate_identifier("col-name").is_err());
        assert!(PostgresConnector::validate_identifier("").is_err());
        assert!(PostgresConnector::validate_identifier("col name").is_err());
    }

    #[test]
    fn test_validate_args_rejects_unsafe_sort_column() {
        let func = PostgresConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "postgres://user:pass@localhost/db".to_string(),
        );
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert(
            "sort_column".to_string(),
            "id; DROP TABLE users".to_string(),
        );

        let result = {
            let sig = func.signature();
            validate_connector_args(&args, &sig).and_then(|_| func.validate_args(&args))
        };
        assert!(result.is_err());
    }

    // --- parse_location tests ---

    #[test]
    fn test_parse_location_normal() {
        let loc = r#"{"sort_col":"id","min":"1","max":"100"}"#;
        let parsed = PostgresConnector::parse_location(loc).expect("should parse");
        assert_eq!(parsed.sort_col, "id");
        assert_eq!(parsed.min, "1");
        assert_eq!(parsed.max, "100");
    }

    #[test]
    fn test_parse_location_negative_values() {
        let loc = r#"{"sort_col":"temperature","min":"-50","max":"-10"}"#;
        let parsed = PostgresConnector::parse_location(loc).expect("should parse");
        assert_eq!(parsed.sort_col, "temperature");
        assert_eq!(parsed.min, "-50");
        assert_eq!(parsed.max, "-10");
    }

    #[test]
    fn test_parse_location_timestamps() {
        let loc =
            r#"{"sort_col":"created_at","min":"2024-01-01T00:00:00","max":"2024-06-30T23:59:59"}"#;
        let parsed = PostgresConnector::parse_location(loc).expect("should parse");
        assert_eq!(parsed.sort_col, "created_at");
        assert_eq!(parsed.min, "2024-01-01T00:00:00");
        assert_eq!(parsed.max, "2024-06-30T23:59:59");
    }

    #[test]
    fn test_parse_location_hyphenated_strings() {
        let loc = r#"{"sort_col":"code","min":"abc-def-123","max":"xyz-789-end"}"#;
        let parsed = PostgresConnector::parse_location(loc).expect("should parse");
        assert_eq!(parsed.sort_col, "code");
        assert_eq!(parsed.min, "abc-def-123");
        assert_eq!(parsed.max, "xyz-789-end");
    }

    #[test]
    fn test_parse_location_missing_sort_col() {
        let loc = r#"{"min":"1","max":"100"}"#;
        assert!(PostgresConnector::parse_location(loc).is_err());
    }

    #[test]
    fn test_parse_location_missing_min() {
        let loc = r#"{"sort_col":"id","max":"100"}"#;
        assert!(PostgresConnector::parse_location(loc).is_err());
    }

    #[test]
    fn test_parse_location_missing_max() {
        let loc = r#"{"sort_col":"id","min":"1"}"#;
        assert!(PostgresConnector::parse_location(loc).is_err());
    }

    #[test]
    fn test_parse_location_invalid_json() {
        assert!(PostgresConnector::parse_location("not json").is_err());
    }

    // --- parse_attached_boundaries tests ---

    #[test]
    fn test_parse_attached_boundaries() {
        let mut attached = HashSet::new();
        attached.insert(r#"{"sort_col":"id","min":"1","max":"100"}"#.to_string());
        attached.insert(r#"{"sort_col":"id","min":"101","max":"200"}"#.to_string());
        attached.insert(r#"{"sort_col":"other_col","min":"a","max":"z"}"#.to_string());

        let boundaries = PostgresConnector::parse_attached_boundaries(&attached, "id");
        assert_eq!(boundaries.len(), 2);
        // Should be sorted
        assert_eq!(boundaries[0], ("1".to_string(), "100".to_string()));
        assert_eq!(boundaries[1], ("101".to_string(), "200".to_string()));
    }

    #[test]
    fn test_parse_attached_boundaries_empty() {
        let attached = HashSet::new();
        let boundaries = PostgresConnector::parse_attached_boundaries(&attached, "id");
        assert!(boundaries.is_empty());
    }

    #[test]
    fn test_parse_attached_boundaries_wrong_column() {
        let mut attached = HashSet::new();
        attached.insert(r#"{"sort_col":"other","min":"1","max":"100"}"#.to_string());

        let boundaries = PostgresConnector::parse_attached_boundaries(&attached, "id");
        assert!(boundaries.is_empty());
    }

    // --- build_location tests ---

    #[test]
    fn test_build_location() {
        let loc = PostgresConnector::build_location("id", "1", "100");
        let parsed: serde_json::Value = serde_json::from_str(&loc).expect("should be valid JSON");
        assert_eq!(parsed["sort_col"], "id");
        assert_eq!(parsed["min"], "1");
        assert_eq!(parsed["max"], "100");
    }

    #[test]
    fn test_build_location_roundtrip() {
        let loc = PostgresConnector::build_location("created_at", "2024-01-01", "2024-12-31");
        let parsed = PostgresConnector::parse_location(&loc).expect("should roundtrip");
        assert_eq!(parsed.sort_col, "created_at");
        assert_eq!(parsed.min, "2024-01-01");
        assert_eq!(parsed.max, "2024-12-31");
    }

    // --- batch_size default ---

    #[test]
    fn test_default_batch_size() {
        let args = HashMap::new();
        assert_eq!(
            PostgresConnector::get_batch_size(&args).expect("should parse"),
            1_000_000
        );
    }

    #[test]
    fn test_custom_batch_size() {
        let mut args = HashMap::new();
        args.insert("batch_size".to_string(), "50000".to_string());
        assert_eq!(
            PostgresConnector::get_batch_size(&args).expect("should parse"),
            50_000
        );
    }
}
