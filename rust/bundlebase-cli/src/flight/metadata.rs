//! Flight SQL metadata handlers for catalogs, schemas, tables, and SQL info.

use super::service::DoGetStream;
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::sql::{
    CommandGetCatalogs, CommandGetCrossReference, CommandGetDbSchemas, CommandGetExportedKeys,
    CommandGetImportedKeys, CommandGetPrimaryKeys, CommandGetSqlInfo, CommandGetTableTypes,
    CommandGetTables, CommandGetXdbcTypeInfo, ProstMessageExt, SqlInfo,
};
use arrow::ipc::writer::IpcWriteOptions;
use arrow_flight::{IpcMessage, SchemaAsIpc};
use arrow_flight::{FlightDescriptor, FlightEndpoint, FlightInfo, Ticket};
use bytes::Bytes;
use futures::TryStreamExt;
use once_cell::sync::Lazy;
use prost::Message;
use std::sync::Arc;
use bundlebase::{CATALOG_NAME, BUNDLE_INFO_SCHEMA, DEFAULT_SCHEMA, catalog_tables};
use tonic::{Request, Response, Status};

/// Wrap a single RecordBatch into a DoGetStream for Flight SQL responses.
fn single_batch_stream(schema: Arc<Schema>, batch: RecordBatch) -> Result<Response<DoGetStream>, Status> {
    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));
    Ok(Response::new(Box::pin(stream)))
}

/// SQL Info data for server capabilities.
pub static SQL_INFO_DATA: Lazy<Vec<(SqlInfo, String)>> = Lazy::new(|| {
    vec![
        (SqlInfo::FlightSqlServerName, "Bundlebase".to_string()),
        (
            SqlInfo::FlightSqlServerVersion,
            env!("CARGO_PKG_VERSION").to_string(),
        ),
    ]
});

/// Get SQL info flight info.
pub fn get_flight_info_sql_info(
    cmd: CommandGetSqlInfo,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("info_name", arrow::datatypes::DataType::UInt32, false),
        arrow::datatypes::Field::new("value", arrow::datatypes::DataType::Utf8, true),
    ]));

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get SQL info data.
pub fn do_get_sql_info() -> Result<Response<DoGetStream>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("info_name", arrow::datatypes::DataType::UInt32, false),
        arrow::datatypes::Field::new("value", arrow::datatypes::DataType::Utf8, true),
    ]));

    let info_names: Vec<u32> = SQL_INFO_DATA.iter().map(|(k, _)| *k as u32).collect();
    let values: Vec<Option<&str>> = SQL_INFO_DATA
        .iter()
        .map(|(_, v)| Some(v.as_str()))
        .collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(arrow::array::UInt32Array::from(info_names)),
            Arc::new(arrow::array::StringArray::from(values)),
        ],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    single_batch_stream(schema, batch)
}

/// Get catalogs flight info.
pub fn get_flight_info_catalogs(
    cmd: CommandGetCatalogs,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
        "catalog_name",
        arrow::datatypes::DataType::Utf8,
        false,
    )]));

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get catalogs data.
pub fn do_get_catalogs() -> Result<Response<DoGetStream>, Status> {
    let schema = Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
        "catalog_name",
        arrow::datatypes::DataType::Utf8,
        false,
    )]));

    // Return single catalog
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(arrow::array::StringArray::from(vec![CATALOG_NAME]))],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    single_batch_stream(schema, batch)
}

/// Get schemas flight info.
pub fn get_flight_info_schemas(
    cmd: CommandGetDbSchemas,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new(
            "db_schema_name",
            arrow::datatypes::DataType::Utf8,
            false,
        ),
    ]));

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get schemas data.
/// `function_namespaces` contains unique namespace names from registered functions.
pub fn do_get_schemas(function_namespaces: &[String]) -> Result<Response<DoGetStream>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new(
            "db_schema_name",
            arrow::datatypes::DataType::Utf8,
            false,
        ),
    ]));

    // Built-in schemas + function namespace schemas
    let mut catalog_names: Vec<Option<&str>> = vec![Some(CATALOG_NAME), Some(CATALOG_NAME)];
    let mut schema_names: Vec<&str> = vec![DEFAULT_SCHEMA, BUNDLE_INFO_SCHEMA];

    for ns in function_namespaces {
        catalog_names.push(Some(CATALOG_NAME));
        schema_names.push(ns.as_str());
    }

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(arrow::array::StringArray::from(catalog_names)),
            Arc::new(arrow::array::StringArray::from(schema_names)),
        ],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    single_batch_stream(schema, batch)
}

/// Get tables flight info.
pub fn get_flight_info_tables(
    cmd: CommandGetTables,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = get_tables_response_schema(cmd.include_schema);

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get the response schema for GetTables, optionally including table_schema column.
fn get_tables_response_schema(include_schema: bool) -> Arc<Schema> {
    let mut fields = vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("db_schema_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("table_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("table_type", arrow::datatypes::DataType::Utf8, false),
    ];
    if include_schema {
        fields.push(arrow::datatypes::Field::new(
            "table_schema",
            arrow::datatypes::DataType::Binary,
            false,
        ));
    }
    Arc::new(Schema::new(fields))
}

/// Get the Arrow schema for a table by schema and table name.
/// If `bundle_schema` is provided, it will be used for the "default.bundle" table.
fn get_table_schema(db_schema: &str, table_name: &str, bundle_schema: Option<&Arc<Schema>>) -> Arc<Schema> {
    if db_schema == DEFAULT_SCHEMA && table_name == "bundle" {
        // Use provided bundle schema if available, otherwise return empty
        return bundle_schema.cloned().unwrap_or_else(|| Arc::new(Schema::empty()));
    }

    if db_schema != BUNDLE_INFO_SCHEMA {
        return Arc::new(Schema::empty());
    }

    if table_name == catalog_tables::HISTORY {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Int32, false),
            arrow::datatypes::Field::new("url", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("author", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("message", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("timestamp", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("change_count", arrow::datatypes::DataType::Int32, false),
        ]))
    } else if table_name == catalog_tables::STATUS {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Int32, false),
            arrow::datatypes::Field::new("change_id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("description", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("operation_count", arrow::datatypes::DataType::Int32, false),
        ]))
    } else if table_name == catalog_tables::DETAILS {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("name", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("description", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("url", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("from", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("version", arrow::datatypes::DataType::Utf8, false),
        ]))
    } else if table_name == catalog_tables::VIEWS {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("name", arrow::datatypes::DataType::Utf8, false),
        ]))
    } else if table_name == catalog_tables::INDEXES {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("column", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("type", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("tokenizer", arrow::datatypes::DataType::Utf8, true),
        ]))
    } else if table_name == catalog_tables::PACKS {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("name", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("join_type", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("expression", arrow::datatypes::DataType::Utf8, true),
        ]))
    } else if table_name == catalog_tables::BLOCKS {
        Arc::new(Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("version", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("pack_id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("pack_name", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("source_id", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("source_location", arrow::datatypes::DataType::Utf8, true),
            arrow::datatypes::Field::new("source_version", arrow::datatypes::DataType::Utf8, true),
        ]))
    } else {
        Arc::new(Schema::empty())
    }
}

/// Serialize an Arrow schema to IPC format bytes.
fn serialize_schema_to_ipc(schema: &Schema) -> Result<Vec<u8>, Status> {
    let options = IpcWriteOptions::default();
    let ipc_message: IpcMessage = SchemaAsIpc::new(schema, &options)
        .try_into()
        .map_err(|e: arrow::error::ArrowError| Status::internal(format!("Failed to encode schema: {}", e)))?;
    Ok(ipc_message.0.to_vec())
}

/// Get tables data.
/// If `bundle_schema` is provided, it will be used for the "default.bundle" table schema.
/// `function_entries` contains (namespace, function_name) pairs for registered functions.
pub fn do_get_tables(
    cmd: CommandGetTables,
    bundle_schema: Option<Arc<Schema>>,
    function_entries: &[bundlebase_common::NamespacedName],
) -> Result<Response<DoGetStream>, Status> {
    let response_schema = get_tables_response_schema(cmd.include_schema);

    // All available tables: (schema, table_name, table_type)
    let mut all_tables: Vec<(String, String, &str)> = vec![
        (DEFAULT_SCHEMA.to_string(), "bundle".to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::HISTORY.to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::STATUS.to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::DETAILS.to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::VIEWS.to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::INDEXES.to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::PACKS.to_string(), "TABLE"),
        (BUNDLE_INFO_SCHEMA.to_string(), catalog_tables::BLOCKS.to_string(), "TABLE"),
    ];

    // Add function entries as FUNCTION type
    for entry in function_entries {
        all_tables.push((entry.namespace.clone(), entry.name.clone(), "FUNCTION"));
    }

    // Filter tables based on request parameters
    let filtered_tables: Vec<_> = all_tables
        .into_iter()
        .filter(|(schema_name, table_name, table_type)| {
            // Filter by catalog (if specified)
            if let Some(ref catalog) = cmd.catalog {
                if catalog != CATALOG_NAME {
                    return false;
                }
            }

            // Filter by schema pattern (if specified)
            if let Some(ref pattern) = cmd.db_schema_filter_pattern {
                if !matches_sql_pattern(pattern, schema_name) {
                    return false;
                }
            }

            // Filter by table name pattern (if specified)
            if let Some(ref pattern) = cmd.table_name_filter_pattern {
                if !matches_sql_pattern(pattern, table_name) {
                    return false;
                }
            }

            // Filter by table types (if specified)
            if !cmd.table_types.is_empty() {
                if !cmd.table_types.contains(&table_type.to_string()) {
                    return false;
                }
            }

            true
        })
        .collect();

    // Build arrays from filtered results
    let catalogs: Vec<Option<&str>> = filtered_tables.iter().map(|_| Some(CATALOG_NAME)).collect();
    let schemas: Vec<Option<&str>> = filtered_tables.iter().map(|(s, _, _)| Some(s.as_str())).collect();
    let tables: Vec<&str> = filtered_tables.iter().map(|(_, t, _)| t.as_str()).collect();
    let types: Vec<&str> = filtered_tables.iter().map(|(_, _, ty)| *ty).collect();

    let mut columns: Vec<Arc<dyn arrow::array::Array>> = vec![
        Arc::new(arrow::array::StringArray::from(catalogs)),
        Arc::new(arrow::array::StringArray::from(schemas)),
        Arc::new(arrow::array::StringArray::from(tables)),
        Arc::new(arrow::array::StringArray::from(types)),
    ];

    // Add table_schema column if requested
    if cmd.include_schema {
        let schema_bytes: Result<Vec<Vec<u8>>, Status> = filtered_tables
            .iter()
            .map(|(db_schema, table_name, _)| {
                let table_schema = get_table_schema(db_schema, table_name, bundle_schema.as_ref());
                serialize_schema_to_ipc(&table_schema)
            })
            .collect();
        let schema_bytes = schema_bytes?;
        columns.push(Arc::new(arrow::array::BinaryArray::from_vec(
            schema_bytes.iter().map(|b| b.as_slice()).collect(),
        )));
    }

    let batch = RecordBatch::try_new(response_schema.clone(), columns)
        .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    single_batch_stream(response_schema, batch)
}

/// Get table types flight info.
pub fn get_flight_info_table_types(
    cmd: CommandGetTableTypes,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
        "table_type",
        arrow::datatypes::DataType::Utf8,
        false,
    )]));

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get table types data.
pub fn do_get_table_types() -> Result<Response<DoGetStream>, Status> {
    let schema = Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
        "table_type",
        arrow::datatypes::DataType::Utf8,
        false,
    )]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(arrow::array::StringArray::from(vec![
            "TABLE", "VIEW",
        ]))],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    single_batch_stream(schema, batch)
}

/// Get primary keys flight info.
pub fn get_flight_info_primary_keys(
    cmd: CommandGetPrimaryKeys,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = primary_keys_schema();

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get primary keys data (returns empty - no primary keys defined).
pub fn do_get_primary_keys() -> Result<Response<DoGetStream>, Status> {
    let schema = primary_keys_schema();
    let batch = RecordBatch::new_empty(schema.clone());
    single_batch_stream(schema, batch)
}

/// Schema for primary keys response.
fn primary_keys_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new(
            "db_schema_name",
            arrow::datatypes::DataType::Utf8,
            true,
        ),
        arrow::datatypes::Field::new("table_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("column_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("key_sequence", arrow::datatypes::DataType::Int32, false),
        arrow::datatypes::Field::new("key_name", arrow::datatypes::DataType::Utf8, true),
    ]))
}

/// Match a value against a SQL LIKE pattern.
/// Supports `%` (any string) and `_` (any single character) wildcards.
fn matches_sql_pattern(pattern: &str, value: &str) -> bool {
    // Handle common patterns without full regex
    if pattern == "%" {
        return true;
    }
    if !pattern.contains('%') && !pattern.contains('_') {
        // Exact match
        return pattern == value;
    }
    if pattern.starts_with('%') && pattern.ends_with('%') && pattern.len() > 2 {
        let middle = &pattern[1..pattern.len() - 1];
        if !middle.contains('%') && !middle.contains('_') {
            return value.contains(middle);
        }
    }
    if pattern.ends_with('%') && !pattern[..pattern.len() - 1].contains('%') {
        let prefix = &pattern[..pattern.len() - 1];
        if !prefix.contains('_') {
            return value.starts_with(prefix);
        }
    }
    if pattern.starts_with('%') && !pattern[1..].contains('%') {
        let suffix = &pattern[1..];
        if !suffix.contains('_') {
            return value.ends_with(suffix);
        }
    }

    // Fall back to recursive matching for complex patterns
    matches_pattern_recursive(pattern.chars().collect::<Vec<_>>().as_slice(), value.chars().collect::<Vec<_>>().as_slice())
}

/// Recursive pattern matching for SQL LIKE patterns.
fn matches_pattern_recursive(pattern: &[char], value: &[char]) -> bool {
    match (pattern.first(), value.first()) {
        (None, None) => true,
        (Some('%'), _) => {
            // % matches zero or more characters
            matches_pattern_recursive(&pattern[1..], value)
                || (!value.is_empty() && matches_pattern_recursive(pattern, &value[1..]))
        }
        (Some('_'), Some(_)) => {
            // _ matches exactly one character
            matches_pattern_recursive(&pattern[1..], &value[1..])
        }
        (Some(p), Some(v)) if *p == *v => {
            matches_pattern_recursive(&pattern[1..], &value[1..])
        }
        _ => false,
    }
}

/// Schema for foreign keys response (used by exported_keys, imported_keys, and cross_reference).
fn foreign_keys_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("pk_catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("pk_db_schema_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("pk_table_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("pk_column_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("fk_catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("fk_db_schema_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("fk_table_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("fk_column_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("key_sequence", arrow::datatypes::DataType::Int32, false),
        arrow::datatypes::Field::new("fk_key_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("pk_key_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("update_rule", arrow::datatypes::DataType::UInt8, false),
        arrow::datatypes::Field::new("delete_rule", arrow::datatypes::DataType::UInt8, false),
    ]))
}

/// Get exported keys flight info.
pub fn get_flight_info_exported_keys(
    cmd: CommandGetExportedKeys,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = foreign_keys_schema();

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get exported keys data (returns empty - Bundlebase has no foreign key constraints).
pub fn do_get_exported_keys() -> Result<Response<DoGetStream>, Status> {
    let schema = foreign_keys_schema();
    let batch = RecordBatch::new_empty(schema.clone());
    single_batch_stream(schema, batch)
}

/// Get imported keys flight info.
pub fn get_flight_info_imported_keys(
    cmd: CommandGetImportedKeys,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = foreign_keys_schema();

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get imported keys data (returns empty - Bundlebase has no foreign key constraints).
pub fn do_get_imported_keys() -> Result<Response<DoGetStream>, Status> {
    let schema = foreign_keys_schema();
    let batch = RecordBatch::new_empty(schema.clone());
    single_batch_stream(schema, batch)
}

/// Get cross reference flight info.
pub fn get_flight_info_cross_reference(
    cmd: CommandGetCrossReference,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = foreign_keys_schema();

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get cross reference data (returns empty - Bundlebase has no foreign key constraints).
pub fn do_get_cross_reference() -> Result<Response<DoGetStream>, Status> {
    let schema = foreign_keys_schema();
    let batch = RecordBatch::new_empty(schema.clone());
    single_batch_stream(schema, batch)
}

/// Schema for XDBC type info response.
fn xdbc_type_info_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("type_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("data_type", arrow::datatypes::DataType::Int32, false),
        arrow::datatypes::Field::new("column_size", arrow::datatypes::DataType::Int32, true),
        arrow::datatypes::Field::new("literal_prefix", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("literal_suffix", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("create_params", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("nullable", arrow::datatypes::DataType::Int32, false),
        arrow::datatypes::Field::new("case_sensitive", arrow::datatypes::DataType::Boolean, false),
        arrow::datatypes::Field::new("searchable", arrow::datatypes::DataType::Int32, false),
        arrow::datatypes::Field::new("unsigned_attribute", arrow::datatypes::DataType::Boolean, true),
        arrow::datatypes::Field::new("fixed_prec_scale", arrow::datatypes::DataType::Boolean, false),
        arrow::datatypes::Field::new("auto_increment", arrow::datatypes::DataType::Boolean, true),
        arrow::datatypes::Field::new("local_type_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new("minimum_scale", arrow::datatypes::DataType::Int32, true),
        arrow::datatypes::Field::new("maximum_scale", arrow::datatypes::DataType::Int32, true),
        arrow::datatypes::Field::new("sql_data_type", arrow::datatypes::DataType::Int32, false),
        arrow::datatypes::Field::new("datetime_subcode", arrow::datatypes::DataType::Int32, true),
        arrow::datatypes::Field::new("num_prec_radix", arrow::datatypes::DataType::Int32, true),
        arrow::datatypes::Field::new("interval_precision", arrow::datatypes::DataType::Int32, true),
    ]))
}

/// Get XDBC type info flight info.
pub fn get_flight_info_xdbc_type_info(
    cmd: CommandGetXdbcTypeInfo,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = xdbc_type_info_schema();

    let ticket = Ticket {
        ticket: cmd.as_any().encode_to_vec().into(),
    };

    let endpoint = FlightEndpoint {
        ticket: Some(ticket),
        location: vec![],
        expiration_time: None,
        app_metadata: Bytes::new(),
    };

    let flight_info = FlightInfo::new()
        .try_with_schema(&schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?
        .with_endpoint(endpoint)
        .with_descriptor(request.into_inner());

    Ok(Response::new(flight_info))
}

/// Get XDBC type info data.
/// Returns basic SQL types supported by Bundlebase.
pub fn do_get_xdbc_type_info() -> Result<Response<DoGetStream>, Status> {
    let schema = xdbc_type_info_schema();

    // SQL type constants (from JDBC/ODBC spec)
    const SQL_VARCHAR: i32 = 12;
    const SQL_INTEGER: i32 = 4;
    const SQL_BIGINT: i32 = -5;
    const SQL_DOUBLE: i32 = 8;
    const SQL_BOOLEAN: i32 = 16;
    const SQL_DATE: i32 = 91;
    const SQL_TIMESTAMP: i32 = 93;
    const SQL_BINARY: i32 = -2;

    // Searchable constants
    const SEARCHABLE: i32 = 3; // Fully searchable

    // Nullable constant
    const NULLABLE: i32 = 1; // Column allows NULLs

    // Type definitions: (name, sql_type, column_size, literal_prefix, literal_suffix, case_sensitive, unsigned, fixed_prec_scale, auto_inc, min_scale, max_scale, num_prec_radix)
    let types: Vec<(
        &str,
        i32,
        Option<i32>,
        Option<&str>,
        Option<&str>,
        bool,
        Option<bool>,
        bool,
        Option<bool>,
        Option<i32>,
        Option<i32>,
        Option<i32>,
    )> = vec![
        ("VARCHAR", SQL_VARCHAR, Some(65535), Some("'"), Some("'"), true, None, false, None, None, None, None),
        ("INTEGER", SQL_INTEGER, Some(10), None, None, false, Some(false), false, Some(false), Some(0), Some(0), Some(10)),
        ("BIGINT", SQL_BIGINT, Some(19), None, None, false, Some(false), false, Some(false), Some(0), Some(0), Some(10)),
        ("DOUBLE", SQL_DOUBLE, Some(15), None, None, false, Some(false), false, Some(false), None, None, Some(10)),
        ("BOOLEAN", SQL_BOOLEAN, Some(1), None, None, false, None, false, None, None, None, None),
        ("DATE", SQL_DATE, Some(10), Some("'"), Some("'"), false, None, false, None, None, None, None),
        ("TIMESTAMP", SQL_TIMESTAMP, Some(29), Some("'"), Some("'"), false, None, false, None, None, None, None),
        ("BINARY", SQL_BINARY, Some(65535), Some("X'"), Some("'"), false, None, false, None, None, None, None),
    ];

    let type_names: Vec<&str> = types.iter().map(|t| t.0).collect();
    let data_types: Vec<i32> = types.iter().map(|t| t.1).collect();
    let column_sizes: Vec<Option<i32>> = types.iter().map(|t| t.2).collect();
    let literal_prefixes: Vec<Option<&str>> = types.iter().map(|t| t.3).collect();
    let literal_suffixes: Vec<Option<&str>> = types.iter().map(|t| t.4).collect();
    let create_params: Vec<Option<&str>> = vec![None; types.len()];
    let nullables: Vec<i32> = vec![NULLABLE; types.len()];
    let case_sensitives: Vec<bool> = types.iter().map(|t| t.5).collect();
    let searchables: Vec<i32> = vec![SEARCHABLE; types.len()];
    let unsigned_attrs: Vec<Option<bool>> = types.iter().map(|t| t.6).collect();
    let fixed_prec_scales: Vec<bool> = types.iter().map(|t| t.7).collect();
    let auto_increments: Vec<Option<bool>> = types.iter().map(|t| t.8).collect();
    let local_type_names: Vec<Option<&str>> = vec![None; types.len()];
    let minimum_scales: Vec<Option<i32>> = types.iter().map(|t| t.9).collect();
    let maximum_scales: Vec<Option<i32>> = types.iter().map(|t| t.10).collect();
    let sql_data_types: Vec<i32> = types.iter().map(|t| t.1).collect(); // Same as data_type
    let datetime_subcodes: Vec<Option<i32>> = vec![None; types.len()];
    let num_prec_radixes: Vec<Option<i32>> = types.iter().map(|t| t.11).collect();
    let interval_precisions: Vec<Option<i32>> = vec![None; types.len()];

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(arrow::array::StringArray::from(type_names)),
            Arc::new(arrow::array::Int32Array::from(data_types)),
            Arc::new(arrow::array::Int32Array::from(column_sizes)),
            Arc::new(arrow::array::StringArray::from(literal_prefixes)),
            Arc::new(arrow::array::StringArray::from(literal_suffixes)),
            Arc::new(arrow::array::StringArray::from(create_params)),
            Arc::new(arrow::array::Int32Array::from(nullables)),
            Arc::new(arrow::array::BooleanArray::from(case_sensitives)),
            Arc::new(arrow::array::Int32Array::from(searchables)),
            Arc::new(arrow::array::BooleanArray::from(unsigned_attrs)),
            Arc::new(arrow::array::BooleanArray::from(fixed_prec_scales)),
            Arc::new(arrow::array::BooleanArray::from(auto_increments)),
            Arc::new(arrow::array::StringArray::from(local_type_names)),
            Arc::new(arrow::array::Int32Array::from(minimum_scales)),
            Arc::new(arrow::array::Int32Array::from(maximum_scales)),
            Arc::new(arrow::array::Int32Array::from(sql_data_types)),
            Arc::new(arrow::array::Int32Array::from(datetime_subcodes)),
            Arc::new(arrow::array::Int32Array::from(num_prec_radixes)),
            Arc::new(arrow::array::Int32Array::from(interval_precisions)),
        ],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    single_batch_stream(schema, batch)
}
