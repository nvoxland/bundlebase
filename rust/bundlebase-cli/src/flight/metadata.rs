//! Flight SQL metadata handlers for catalogs, schemas, tables, and SQL info.

use super::execution::DoGetStream;
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::sql::{
    CommandGetCatalogs, CommandGetDbSchemas, CommandGetPrimaryKeys, CommandGetSqlInfo,
    CommandGetTableTypes, CommandGetTables, ProstMessageExt, SqlInfo,
};
use arrow_flight::{FlightDescriptor, FlightEndpoint, FlightInfo, Ticket};
use bytes::Bytes;
use futures::TryStreamExt;
use once_cell::sync::Lazy;
use prost::Message;
use std::sync::Arc;
use tonic::{Request, Response, Status};

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

    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));

    Ok(Response::new(Box::pin(stream)))
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

    // Return single catalog "bundlebase"
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(arrow::array::StringArray::from(vec!["bundlebase"]))],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));

    Ok(Response::new(Box::pin(stream)))
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
pub fn do_get_schemas() -> Result<Response<DoGetStream>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new(
            "db_schema_name",
            arrow::datatypes::DataType::Utf8,
            false,
        ),
    ]));

    // Return single schema "public"
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(arrow::array::StringArray::from(vec![Some("bundlebase")])),
            Arc::new(arrow::array::StringArray::from(vec!["public"])),
        ],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));

    Ok(Response::new(Box::pin(stream)))
}

/// Get tables flight info.
pub fn get_flight_info_tables(
    cmd: CommandGetTables,
    request: Request<FlightDescriptor>,
) -> Result<Response<FlightInfo>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new(
            "db_schema_name",
            arrow::datatypes::DataType::Utf8,
            true,
        ),
        arrow::datatypes::Field::new("table_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("table_type", arrow::datatypes::DataType::Utf8, false),
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

/// Get tables data.
pub fn do_get_tables() -> Result<Response<DoGetStream>, Status> {
    let schema = Arc::new(Schema::new(vec![
        arrow::datatypes::Field::new("catalog_name", arrow::datatypes::DataType::Utf8, true),
        arrow::datatypes::Field::new(
            "db_schema_name",
            arrow::datatypes::DataType::Utf8,
            true,
        ),
        arrow::datatypes::Field::new("table_name", arrow::datatypes::DataType::Utf8, false),
        arrow::datatypes::Field::new("table_type", arrow::datatypes::DataType::Utf8, false),
    ]));

    // Return the bundle table
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(arrow::array::StringArray::from(vec![Some("bundlebase")])),
            Arc::new(arrow::array::StringArray::from(vec![Some("public")])),
            Arc::new(arrow::array::StringArray::from(vec!["bundle"])),
            Arc::new(arrow::array::StringArray::from(vec!["TABLE"])),
        ],
    )
    .map_err(|e| Status::internal(format!("Failed to create batch: {}", e)))?;

    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));

    Ok(Response::new(Box::pin(stream)))
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

    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));

    Ok(Response::new(Box::pin(stream)))
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

    // Return empty batch (no primary keys)
    let batch = RecordBatch::new_empty(schema.clone());

    let stream = FlightDataEncoderBuilder::new()
        .with_schema(schema)
        .build(futures::stream::once(async { Ok(batch) }))
        .map_err(|e| Status::from_error(Box::new(e)));

    Ok(Response::new(Box::pin(stream)))
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
