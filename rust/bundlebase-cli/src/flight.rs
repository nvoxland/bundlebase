//! Arrow Flight SQL server implementation for bundlebase.
//!
//! This module provides an Arrow Flight SQL service that allows JDBC clients
//! to connect and execute SQL queries against bundlebase bundles.

use crate::auth::BundlebaseAuthenticator;
use crate::sql_executor::{self, SqlResult};
use crate::state::BundleState;
use arrow::datatypes::{Schema, SchemaRef};
// Note: IpcDataGenerator is available but we use FlightInfo for schema encoding
use arrow::record_batch::RecordBatch;
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_server::{FlightService, FlightServiceServer};
use arrow_flight::sql::server::FlightSqlService;
use arrow_flight::sql::{
    ActionClosePreparedStatementRequest, ActionCreatePreparedStatementRequest,
    ActionCreatePreparedStatementResult, Any, CommandGetCatalogs, CommandGetDbSchemas,
    CommandGetPrimaryKeys, CommandGetSqlInfo, CommandGetTableTypes, CommandGetTables,
    CommandPreparedStatementQuery, CommandStatementQuery, ProstMessageExt, SqlInfo,
    TicketStatementQuery,
};
use arrow_flight::{
    Action, FlightData, FlightDescriptor, FlightEndpoint, FlightInfo, HandshakeRequest,
    HandshakeResponse, Ticket,
};
use bundlebase::bundle::BundleFacade;
use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use prost::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;
use uuid::Uuid;

/// SQL Info data for server capabilities.
static SQL_INFO_DATA: Lazy<Vec<(SqlInfo, String)>> = Lazy::new(|| {
    vec![
        (SqlInfo::FlightSqlServerName, "Bundlebase".to_string()),
        (
            SqlInfo::FlightSqlServerVersion,
            env!("CARGO_PKG_VERSION").to_string(),
        ),
    ]
});

/// Stored prepared statement information.
struct PreparedStatement {
    sql: String,
    schema: SchemaRef,
}

/// The bundlebase Flight SQL service implementation.
pub struct BundlebaseFlightSqlService {
    state: Arc<BundleState>,
    authenticator: BundlebaseAuthenticator,
    prepared_statements: Arc<RwLock<HashMap<String, PreparedStatement>>>,
}

impl BundlebaseFlightSqlService {
    /// Create a new Flight SQL service with the given bundle state.
    pub fn new(state: Arc<BundleState>) -> Self {
        Self {
            state,
            authenticator: BundlebaseAuthenticator::default(),
            prepared_statements: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new Flight SQL service with custom authenticator.
    pub fn with_authenticator(
        state: Arc<BundleState>,
        authenticator: BundlebaseAuthenticator,
    ) -> Self {
        Self {
            state,
            authenticator,
            prepared_statements: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the schema for a SQL query by planning it.
    async fn get_query_schema(&self, sql: &str) -> Result<SchemaRef, Status> {
        // First check if it's a bundlebase command with known schema
        if let Some(schema) = sql_executor::get_command_schema(sql) {
            return Ok(schema);
        }

        // For standard SQL, we need to plan the query to get the schema
        let builder = {
            let guard = self.state.bundle.read();
            guard.clone()
        };

        // Use select to plan the query and get the schema
        let result_builder = builder
            .select(sql, vec![])
            .await
            .map_err(|e| Status::internal(format!("Failed to plan query: {}", e)))?;

        let df = result_builder
            .dataframe()
            .await
            .map_err(|e| Status::internal(format!("Failed to get dataframe: {}", e)))?;

        Ok(df.schema().inner().clone())
    }
}

#[tonic::async_trait]
impl FlightSqlService for BundlebaseFlightSqlService {
    type FlightService = BundlebaseFlightSqlService;

    /// Handle handshake for authentication.
    async fn do_handshake(
        &self,
        request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<
        Response<Pin<Box<dyn Stream<Item = Result<HandshakeResponse, Status>> + Send>>>,
        Status,
    > {
        let mut request_stream = request.into_inner();

        // Get the first handshake message
        let handshake_request = request_stream
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("No handshake request received"))?
            .map_err(|e| Status::internal(format!("Failed to receive handshake: {}", e)))?;

        // Try to validate as Basic Auth (format: username:password)
        let payload = handshake_request.payload;
        let auth_str = String::from_utf8(payload.to_vec())
            .map_err(|_| Status::unauthenticated("Invalid auth payload encoding"))?;

        // Parse username:password
        let parts: Vec<&str> = auth_str.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(Status::unauthenticated(
                "Invalid auth format, expected username:password",
            ));
        }

        let username = parts[0];
        let password = parts[1];

        // Validate credentials
        if !self.authenticator.validate(username, password) {
            return Err(Status::unauthenticated("Invalid credentials"));
        }

        // Return success response with a token
        let token = format!("token-{}", Uuid::new_v4());
        let response = HandshakeResponse {
            protocol_version: 1,
            payload: Bytes::from(token),
        };

        let stream = futures::stream::once(async { Ok(response) });
        Ok(Response::new(Box::pin(stream)))
    }

    /// Create a prepared statement.
    async fn do_action_create_prepared_statement(
        &self,
        query: ActionCreatePreparedStatementRequest,
        _request: Request<Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        let sql = query.query.clone();
        let handle = Uuid::new_v4().to_string();

        info!("Creating prepared statement: {} -> {}", handle, sql);

        // Get schema by planning query (not executing)
        let schema = self.get_query_schema(&sql).await?;

        // Store prepared statement
        self.prepared_statements.write().insert(
            handle.clone(),
            PreparedStatement {
                sql,
                schema: schema.clone(),
            },
        );

        // Encode schema to IPC format
        let dataset_schema = encode_schema(&schema)?;
        let parameter_schema = encode_schema(&Arc::new(Schema::empty()))?;

        Ok(ActionCreatePreparedStatementResult {
            prepared_statement_handle: handle.into(),
            dataset_schema,
            parameter_schema,
        })
    }

    /// Close a prepared statement.
    async fn do_action_close_prepared_statement(
        &self,
        query: ActionClosePreparedStatementRequest,
        _request: Request<Action>,
    ) -> Result<(), Status> {
        let handle = String::from_utf8(query.prepared_statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid prepared statement handle"))?;

        info!("Closing prepared statement: {}", handle);

        self.prepared_statements.write().remove(&handle);
        Ok(())
    }

    /// Get flight info for a prepared statement query.
    async fn get_flight_info_prepared_statement(
        &self,
        cmd: CommandPreparedStatementQuery,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let handle = String::from_utf8(cmd.prepared_statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid prepared statement handle"))?;

        let schema = {
            let stmts = self.prepared_statements.read();
            stmts
                .get(&handle)
                .ok_or_else(|| Status::not_found("Prepared statement not found"))?
                .schema
                .clone()
        };

        // Create a ticket that contains the prepared statement command
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

    /// Execute a prepared statement query and return results.
    async fn do_get_prepared_statement(
        &self,
        cmd: CommandPreparedStatementQuery,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let handle = String::from_utf8(cmd.prepared_statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid prepared statement handle"))?;

        let sql = {
            let stmts = self.prepared_statements.read();
            stmts
                .get(&handle)
                .ok_or_else(|| Status::not_found("Prepared statement not found"))?
                .sql
                .clone()
        };

        info!("Executing prepared statement: {} -> {}", handle, sql);

        execute_query_streaming(&self.state, sql).await
    }

    /// Handle direct SQL statement queries (non-prepared).
    async fn get_flight_info_statement(
        &self,
        cmd: CommandStatementQuery,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let sql = cmd.query.clone();

        info!("Getting flight info for statement: {}", sql);

        // Get schema by planning the query
        let schema = self.get_query_schema(&sql).await?;

        // Create a TicketStatementQuery with SQL in statement_handle
        let ticket_stmt = TicketStatementQuery {
            statement_handle: sql.into_bytes().into(),
        };
        let ticket = Ticket {
            ticket: ticket_stmt.as_any().encode_to_vec().into(),
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

    /// Execute a direct SQL statement query.
    async fn do_get_statement(
        &self,
        ticket: TicketStatementQuery,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // Decode SQL from statement_handle (we stored the SQL bytes directly)
        let sql = String::from_utf8(ticket.statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid statement handle encoding"))?;

        info!("Executing direct statement: {}", sql);

        execute_query_streaming(&self.state, sql).await
    }

    /// Fallback handler for unknown ticket types (backward compatibility).
    async fn do_get_fallback(
        &self,
        request: Request<Ticket>,
        _message: Any,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // Try raw SQL in ticket (existing behavior for backward compatibility)
        let ticket = request.into_inner();
        if let Ok(sql) = String::from_utf8(ticket.ticket.to_vec()) {
            info!("Executing fallback query: {}", sql);
            return execute_query_streaming(&self.state, sql).await;
        }
        Err(Status::unimplemented("Unknown ticket format"))
    }

    /// Get SQL info (server capabilities).
    async fn get_flight_info_sql_info(
        &self,
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
    async fn do_get_sql_info(
        &self,
        _cmd: CommandGetSqlInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // Return basic SQL info
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

    /// Get catalogs info.
    async fn get_flight_info_catalogs(
        &self,
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
    async fn do_get_catalogs(
        &self,
        _cmd: CommandGetCatalogs,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
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

    /// Get schemas info.
    async fn get_flight_info_schemas(
        &self,
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
    async fn do_get_schemas(
        &self,
        _cmd: CommandGetDbSchemas,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
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

    /// Get tables info.
    async fn get_flight_info_tables(
        &self,
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
    async fn do_get_tables(
        &self,
        _cmd: CommandGetTables,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
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

    /// Get table types info.
    async fn get_flight_info_table_types(
        &self,
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
    async fn do_get_table_types(
        &self,
        _cmd: CommandGetTableTypes,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
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

    /// Get primary keys info (stub).
    async fn get_flight_info_primary_keys(
        &self,
        cmd: CommandGetPrimaryKeys,
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
            arrow::datatypes::Field::new("column_name", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("key_sequence", arrow::datatypes::DataType::Int32, false),
            arrow::datatypes::Field::new("key_name", arrow::datatypes::DataType::Utf8, true),
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

    /// Get primary keys data (returns empty - no primary keys defined).
    async fn do_get_primary_keys(
        &self,
        _cmd: CommandGetPrimaryKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let schema = Arc::new(Schema::new(vec![
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
        ]));

        // Return empty batch (no primary keys)
        let batch = RecordBatch::new_empty(schema.clone());

        let stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(futures::stream::once(async { Ok(batch) }))
            .map_err(|e| Status::from_error(Box::new(e)));

        Ok(Response::new(Box::pin(stream)))
    }

    /// Register SQL info (not needed for read-only server).
    async fn register_sql_info(&self, _id: i32, _result: &SqlInfo) {}
}

/// Stream type for FlightData responses.
type DoGetStream = Pin<Box<dyn Stream<Item = Result<FlightData, Status>> + Send>>;

/// Execute a query and return a streaming FlightData response.
async fn execute_query_streaming(
    state: &Arc<BundleState>,
    sql: String,
) -> Result<Response<DoGetStream>, Status> {
    match sql_executor::execute_sql(state, &sql).await {
        Ok(SqlResult::Stream(record_stream)) => {
            // Get schema from the stream
            let schema = record_stream.schema();

            // Convert the SendableRecordBatchStream to a stream of Results
            let batch_stream = record_stream
                .map(|result| result.map_err(|e| FlightError::ExternalError(Box::new(e))));

            // Use FlightDataEncoder to properly encode the stream
            let flight_stream = FlightDataEncoderBuilder::new()
                .with_schema(schema)
                .build(batch_stream)
                .map_err(|e| Status::from_error(Box::new(e)));

            Ok(Response::new(Box::pin(flight_stream)))
        }
        Ok(SqlResult::Output(output)) => {
            // BundleCommand - convert to record batch and stream
            let batch = output
                .to_record_batch()
                .map_err(|e| Status::internal(format!("Failed to convert output: {}", e)))?;

            let schema = batch.schema();

            let flight_stream = FlightDataEncoderBuilder::new()
                .with_schema(schema)
                .build(futures::stream::once(async { Ok(batch) }))
                .map_err(|e| Status::from_error(Box::new(e)));

            Ok(Response::new(Box::pin(flight_stream)))
        }
        Err(e) => Err(Status::internal(format!("Failed to execute query: {}", e))),
    }
}

/// Encode a schema to IPC format bytes for Flight SQL.
fn encode_schema(schema: &SchemaRef) -> Result<Bytes, Status> {
    // Use FlightInfo's schema encoding which produces the correct IPC format
    let flight_info = FlightInfo::new().try_with_schema(schema).map_err(|e| {
        Status::internal(format!("Failed to encode schema: {}", e))
    })?;

    Ok(Bytes::from(flight_info.schema))
}

// Re-export for backward compatibility
pub use BundlebaseFlightSqlService as BundlebaseFlightService;

/// Start the Flight SQL server.
///
/// This function starts an Arrow Flight SQL server on the specified address
/// and blocks until the server is shut down.
///
/// # Arguments
///
/// * `state` - The shared bundle state
/// * `addr` - The address to bind to (e.g., "0.0.0.0:50051")
///
/// # Returns
///
/// * `Ok(())` - Server shut down cleanly
/// * `Err(BundlebaseError)` - Server failed to start or encountered an error
pub async fn start(
    state: Arc<BundleState>,
    addr: SocketAddr,
) -> Result<(), bundlebase::BundlebaseError> {
    info!("Starting Arrow Flight SQL server on {}", addr);

    let flight_service = BundlebaseFlightSqlService::new(state);

    Server::builder()
        .add_service(FlightServiceServer::new(flight_service))
        .serve(addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase::BundleBuilder;

    #[tokio::test]
    async fn test_flight_sql_service_with_memory_bundle() {
        // Create a bundle and wrap it in a Flight SQL service
        let builder = BundleBuilder::create("memory:///flight_test", None)
            .await
            .expect("Failed to create bundle");

        let _service = BundlebaseFlightSqlService::new(Arc::new(BundleState::new(builder)));

        // Service should instantiate successfully
    }

    #[tokio::test]
    async fn test_prepared_statement_lifecycle() {
        let builder = BundleBuilder::create("memory:///prepared_stmt_test", None)
            .await
            .expect("Failed to create bundle");

        let service = BundlebaseFlightSqlService::new(Arc::new(BundleState::new(builder)));

        // Verify we can create and close prepared statements via the internal state
        let handle = "test-handle".to_string();
        let schema = Arc::new(Schema::new(vec![arrow::datatypes::Field::new(
            "num",
            arrow::datatypes::DataType::Int64,
            false,
        )]));

        // Insert a prepared statement
        service.prepared_statements.write().insert(
            handle.clone(),
            PreparedStatement {
                sql: "SELECT 1".to_string(),
                schema: schema.clone(),
            },
        );

        // Verify it exists
        assert!(service.prepared_statements.read().contains_key(&handle));

        // Remove it
        service.prepared_statements.write().remove(&handle);

        // Verify it's gone
        assert!(!service.prepared_statements.read().contains_key(&handle));
    }
}
