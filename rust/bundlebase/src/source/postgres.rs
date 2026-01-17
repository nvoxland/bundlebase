//! PostgreSQL source function.
//!
//! Extracts data from a PostgreSQL database query, partitioning
//! results into fixed-size chunks based on a sort column.
//!
//! Arguments:
//! - `url` (required): PostgreSQL connection URL (postgres://user:pass@host:port/dbname)
//! - `query` (required): SQL query to execute
//! - `sort_column` (required): Column to ORDER BY and partition on
//! - `batch_size` (optional): Rows per output file (default: 10000)
//! - `mode` (optional): Sync mode: 'add' (default), 'update', or 'sync'

use super::source_function::{
    ArgSpec, AttachedFileInfo, DiscoveredLocation, MaterializedData, FetchAction,
    MaterializeResult, SourceFunction, SyncMode,
};
use crate::io::{IOReadWriteDir, WriteResult};
use crate::{BundleConfig, BundlebaseError};
use arrow::array::{
    ArrayRef, BooleanBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, StringBuilder, TimestampMicrosecondBuilder,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_postgres::{Client, NoTls, Row};
use url::Url;

/// PostgreSQL source function.
///
/// Extracts data from a PostgreSQL query, partitioned into chunks by row count.
pub struct PostgresFunction;

/// Represents a chunk of data with its value range.
#[derive(Debug, Clone)]
struct DataChunk {
    /// Source location identifier (sort_column:min-max)
    source_location: String,
    /// The actual rows in this chunk
    rows: Vec<Row>,
}

impl PostgresFunction {
    /// Connect to PostgreSQL database.
    async fn connect(url: &str) -> Result<Client, BundlebaseError> {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .map_err(|e| BundlebaseError::from(format!("Failed to connect to PostgreSQL: {}", e)))?;

        // Spawn the connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("PostgreSQL connection error: {}", e);
            }
        });

        Ok(client)
    }

    /// Execute query and partition results into chunks using keyset pagination.
    /// This is memory-efficient as it only holds one batch at a time.
    async fn execute_query_chunked(
        client: &Client,
        query: &str,
        sort_column: &str,
        batch_size: usize,
    ) -> Result<Vec<DataChunk>, BundlebaseError> {
        let base_query = query.trim_end_matches(';');
        let mut chunks = Vec::new();
        let mut last_max_value: Option<String> = None;

        loop {
            // Build query with keyset pagination
            let paginated_query = match &last_max_value {
                None => {
                    // First batch - no WHERE clause needed
                    format!(
                        "SELECT * FROM ({}) AS _q ORDER BY {} ASC LIMIT {}",
                        base_query, sort_column, batch_size
                    )
                }
                Some(last_max) => {
                    // Subsequent batches - use keyset pagination
                    format!(
                        "SELECT * FROM ({}) AS _q WHERE {} > '{}' ORDER BY {} ASC LIMIT {}",
                        base_query, sort_column, last_max, sort_column, batch_size
                    )
                }
            };

            let rows = client
                .query(&paginated_query, &[])
                .await
                .map_err(|e| BundlebaseError::from(format!("Query failed: {}", e)))?;

            if rows.is_empty() {
                break;
            }

            // Find the sort column index
            let columns = rows[0].columns();
            let sort_col_idx = columns
                .iter()
                .position(|c| c.name() == sort_column)
                .ok_or_else(|| {
                    BundlebaseError::from(format!(
                        "Sort column '{}' not found in query results",
                        sort_column
                    ))
                })?;

            let min_value = Self::get_value_as_string(&rows[0], sort_col_idx)?;
            let max_value = Self::get_value_as_string(&rows[rows.len() - 1], sort_col_idx)?;

            let source_location = format!("{}:{}-{}", sort_column, min_value, max_value);

            // Update last_max_value for next iteration
            last_max_value = Some(max_value.clone());

            chunks.push(DataChunk {
                source_location,
                rows,
            });

            // If we got fewer rows than batch_size, we're done
            if chunks.last().map(|c| c.rows.len()).unwrap_or(0) < batch_size {
                break;
            }
        }

        Ok(chunks)
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

    /// Write a chunk to parquet and return WriteResult with file and hash.
    async fn write_chunk_to_parquet(
        chunk: &DataChunk,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<WriteResult, BundlebaseError> {
        let batch = Self::rows_to_record_batch(&chunk.rows)?;

        // Write to in-memory buffer
        let mut buffer = Vec::new();
        {
            let props = WriterProperties::builder().build();
            let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))
                .map_err(|e| BundlebaseError::from(format!("Failed to create parquet writer: {}", e)))?;
            writer
                .write(&batch)
                .map_err(|e| BundlebaseError::from(format!("Failed to write batch: {}", e)))?;
            writer
                .close()
                .map_err(|e| BundlebaseError::from(format!("Failed to close writer: {}", e)))?;
        }

        // Write with content-addressed naming
        let suffix = format!("{}.parquet", chunk.source_location.replace(':', "_").replace('-', "_"));
        let data_stream = Box::pin(stream::once(async { Ok(Bytes::from(buffer)) }));
        data_dir.write_stream(data_stream, &suffix).await
    }

    /// Parse batch_size from args with default.
    fn get_batch_size(args: &HashMap<String, String>) -> Result<usize, BundlebaseError> {
        match args.get("batch_size") {
            Some(s) => s
                .parse::<usize>()
                .map_err(|_| BundlebaseError::from("batch_size must be a positive integer")),
            None => Ok(10000),
        }
    }

    /// Re-fetch data for an existing range, keeping the same boundaries.
    async fn refetch_range(
        client: &Client,
        query: &str,
        sort_column: &str,
        source_location: &str,
    ) -> Result<Option<DataChunk>, BundlebaseError> {
        // Parse source_location: "sort_column:min-max"
        let parts: Vec<&str> = source_location.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid source_location format: {}", source_location).into());
        }

        let range_parts: Vec<&str> = parts[1].splitn(2, '-').collect();
        if range_parts.len() != 2 {
            return Err(format!("Invalid range in source_location: {}", source_location).into());
        }

        let min_value = range_parts[0];
        let max_value = range_parts[1];

        // Re-fetch this exact range
        let ranged_query = format!(
            "SELECT * FROM ({}) AS _q WHERE {} >= '{}' AND {} <= '{}' ORDER BY {} ASC",
            query.trim_end_matches(';'),
            sort_column,
            min_value,
            sort_column,
            max_value,
            sort_column
        );

        let rows = client
            .query(&ranged_query, &[])
            .await
            .map_err(|e| BundlebaseError::from(format!("Query failed: {}", e)))?;

        if rows.is_empty() {
            return Ok(None);
        }

        Ok(Some(DataChunk {
            source_location: source_location.to_string(),
            rows,
        }))
    }
}

#[async_trait]
impl SourceFunction for PostgresFunction {
    fn name(&self) -> &str {
        "postgres"
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec {
                name: "url",
                description: "PostgreSQL connection URL (postgres://user:pass@host:port/dbname)",
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
                default: Some("10000"),
            },
            ArgSpec {
                name: "mode",
                description: "Sync mode: 'add' (default), 'update', or 'sync'",
                required: false,
                default: Some("add"),
            },
        ]
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        self.default_validate_args(args)?;

        // Validate URL format
        let url = args
            .get("url")
            .ok_or_else(|| BundlebaseError::from("postgres source requires 'url' argument"))?;
        if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
            return Err("url must be a PostgreSQL connection URL (postgres://...)".into());
        }

        // Validate batch_size if provided
        if let Some(batch_size) = args.get("batch_size") {
            batch_size.parse::<usize>().map_err(|_| {
                BundlebaseError::from("batch_size must be a positive integer")
            })?;
        }

        // Validate mode if provided
        if let Some(mode) = args.get("mode") {
            SyncMode::from_arg(mode)?;
        }

        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let url = args.get("url").ok_or("url is required")?;
        let query = args.get("query").ok_or("query is required")?;
        let sort_column = args.get("sort_column").ok_or("sort_column is required")?;
        let batch_size = Self::get_batch_size(args)?;

        let client = Self::connect(url).await?;
        let chunks = Self::execute_query_chunked(&client, query, sort_column, batch_size).await?;

        // Filter out already-attached locations
        let locations = chunks
            .into_iter()
            .filter(|chunk| !attached_locations.contains(&chunk.source_location))
            .map(|chunk| {
                // Create a pseudo-URL for the source location
                let pseudo_url = Url::parse(&format!("postgres://query/{}", chunk.source_location))
                    .unwrap_or_else(|_| Url::parse("postgres://query/unknown").expect("valid url"));
                DiscoveredLocation {
                    url: pseudo_url,
                    source_location: chunk.source_location,
                }
            })
            .collect();

        Ok(locations)
    }

    async fn materialize(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        data_dir: &dyn IOReadWriteDir,
        _config: &Arc<BundleConfig>,
    ) -> Result<MaterializeResult, BundlebaseError> {
        let url = args.get("url").ok_or("url is required")?;
        let query = args.get("query").ok_or("query is required")?;
        let sort_column = args.get("sort_column").ok_or("sort_column is required")?;

        // Parse the source_location to get min/max values
        // Format: sort_column:min-max
        let parts: Vec<&str> = location.source_location.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid source_location format: {}",
                location.source_location
            )
            .into());
        }

        let range_parts: Vec<&str> = parts[1].splitn(2, '-').collect();
        if range_parts.len() != 2 {
            return Err(format!(
                "Invalid range format in source_location: {}",
                location.source_location
            )
            .into());
        }

        let min_value = range_parts[0];
        let max_value = range_parts[1];

        // Execute query with WHERE clause to fetch this specific range
        let ranged_query = format!(
            "SELECT * FROM ({}) AS _q WHERE {} >= '{}' AND {} <= '{}' ORDER BY {} ASC",
            query.trim_end_matches(';'),
            sort_column,
            min_value,
            sort_column,
            max_value,
            sort_column
        );

        let client = Self::connect(url).await?;
        let rows = client
            .query(&ranged_query, &[])
            .await
            .map_err(|e| BundlebaseError::from(format!("Query failed: {}", e)))?;

        if rows.is_empty() {
            return Err(format!(
                "No rows returned for range {} in source_location",
                location.source_location
            )
            .into());
        }

        let chunk = DataChunk {
            source_location: location.source_location.clone(),
            rows,
        };

        let result = Self::write_chunk_to_parquet(&chunk, data_dir).await?;
        Ok(MaterializeResult {
            file: result.file,
            hash: result.hash,
        })
    }

    async fn fetch_with_mode(
        &self,
        args: &HashMap<String, String>,
        attached_files: &HashMap<String, AttachedFileInfo>,
        data_dir: &dyn IOReadWriteDir,
        _config: Arc<BundleConfig>,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        let url = args.get("url").ok_or("url is required")?;
        let query = args.get("query").ok_or("query is required")?;
        let sort_column = args.get("sort_column").ok_or("sort_column is required")?;
        let batch_size = Self::get_batch_size(args)?;

        let client = Self::connect(url).await?;
        let mut actions = Vec::new();

        // For Update/Sync mode: re-fetch existing ranges (keeping same boundaries)
        if mode == SyncMode::Update || mode == SyncMode::Sync {
            for (source_location, _attached_info) in attached_files {
                match Self::refetch_range(&client, query, sort_column, source_location).await? {
                    Some(chunk) => {
                        let result = Self::write_chunk_to_parquet(&chunk, data_dir).await?;
                        // Use relative path if file is in data_dir, otherwise full URL
                        let attach_location = data_dir
                            .relative_path(result.file.as_ref())
                            .unwrap_or_else(|_| result.file.url().to_string());
                        // For Postgres, source_url is the attach_location since there's no remote file
                        actions.push(FetchAction::Replace {
                            old_source_location: source_location.clone(),
                            data: MaterializedData {
                                attach_location: attach_location.clone(),
                                source_location: source_location.clone(),
                                source_url: attach_location,
                                hash: result.hash,
                            },
                        });
                    }
                    None => {
                        // Range is now empty - in Sync mode, remove it
                        if mode == SyncMode::Sync {
                            actions.push(FetchAction::Remove {
                                source_location: source_location.clone(),
                            });
                        }
                    }
                }
            }
        }

        // For Add mode (or discovering new data): partition new data into chunks
        let chunks =
            Self::execute_query_chunked(&client, query, sort_column, batch_size).await?;

        for chunk in chunks {
            let source_location = chunk.source_location.clone();

            // Skip if already attached (handled above for Update/Sync, skip for Add)
            if attached_files.contains_key(&source_location) {
                continue;
            }

            // New chunk - add it
            let result = Self::write_chunk_to_parquet(&chunk, data_dir).await?;
            // Use relative path if file is in data_dir, otherwise full URL
            let attach_location = data_dir
                .relative_path(result.file.as_ref())
                .unwrap_or_else(|_| result.file.url().to_string());
            // For Postgres, source_url is the attach_location since there's no remote file
            actions.push(FetchAction::Add(MaterializedData {
                attach_location: attach_location.clone(),
                source_location,
                source_url: attach_location,
                hash: result.hash,
            }));
        }

        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let func = PostgresFunction;
        assert_eq!(func.name(), "postgres");
    }

    #[test]
    fn test_arg_specs() {
        let func = PostgresFunction;
        let specs = func.arg_specs();
        assert_eq!(specs.len(), 5);
        assert!(specs.iter().any(|s| s.name == "url" && s.required));
        assert!(specs.iter().any(|s| s.name == "query" && s.required));
        assert!(specs.iter().any(|s| s.name == "sort_column" && s.required));
        assert!(specs.iter().any(|s| s.name == "batch_size" && !s.required));
        assert!(specs.iter().any(|s| s.name == "mode" && !s.required));
    }

    #[test]
    fn test_validate_args_with_valid_url() {
        let func = PostgresFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "postgres://user:pass@localhost/db".to_string());
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_with_postgresql_url() {
        let func = PostgresFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "postgresql://user:pass@localhost/db".to_string());
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_url() {
        let func = PostgresFunction;
        let mut args = HashMap::new();
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_args_invalid_url() {
        let func = PostgresFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "mysql://user:pass@localhost/db".to_string());
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PostgreSQL"));
    }

    #[test]
    fn test_validate_args_invalid_batch_size() {
        let func = PostgresFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "postgres://user:pass@localhost/db".to_string());
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        args.insert("batch_size".to_string(), "not_a_number".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_args_invalid_mode() {
        let func = PostgresFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "postgres://user:pass@localhost/db".to_string());
        args.insert("query".to_string(), "SELECT * FROM users".to_string());
        args.insert("sort_column".to_string(), "id".to_string());
        args.insert("mode".to_string(), "invalid_mode".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
    }
}
