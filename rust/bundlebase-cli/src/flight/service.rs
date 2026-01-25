//! Core Flight SQL service implementation.

use super::execution::execute_query_streaming;
use super::metadata;
use super::prepared_statements::{PreparedStatement, PreparedStatementStore};
use crate::auth::BundlebaseAuthenticator;
use crate::sql_executor;
use crate::state::BundleState;
use arrow::datatypes::{Schema, SchemaRef};
use arrow_flight::flight_service_server::FlightService;
use arrow_flight::sql::server::FlightSqlService;
use arrow_flight::sql::server::PeekableFlightDataStream;
use arrow_flight::sql::{
    ActionClosePreparedStatementRequest, ActionCreatePreparedStatementRequest,
    ActionCreatePreparedStatementResult, Any, CommandGetCatalogs, CommandGetDbSchemas,
    CommandGetPrimaryKeys, CommandGetSqlInfo, CommandGetTableTypes, CommandGetTables,
    CommandPreparedStatementQuery, CommandPreparedStatementUpdate, CommandStatementQuery,
    ProstMessageExt, SqlInfo, TicketStatementQuery,
};
use arrow_flight::{
    FlightDescriptor, FlightEndpoint, FlightInfo, HandshakeRequest, HandshakeResponse, Ticket,
};
use bundlebase::bundle::BundleFacade;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use prost::Message;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;
use uuid::Uuid;

/// The bundlebase Flight SQL service implementation.
pub struct BundlebaseFlightSqlService {
    state: Arc<BundleState>,
    authenticator: BundlebaseAuthenticator,
    prepared_statements: PreparedStatementStore,
}

impl BundlebaseFlightSqlService {
    /// Create a new Flight SQL service with the given bundle state.
    pub fn new(state: Arc<BundleState>) -> Self {
        Self {
            state,
            authenticator: BundlebaseAuthenticator::default(),
            prepared_statements: super::prepared_statements::new_store(),
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
            prepared_statements: super::prepared_statements::new_store(),
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

    /// Get the prepared statements store (for testing).
    #[cfg(test)]
    pub fn prepared_statements(&self) -> &PreparedStatementStore {
        &self.prepared_statements
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
        _request: Request<arrow_flight::Action>,
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
        _request: Request<arrow_flight::Action>,
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

    /// Execute a prepared statement update (DML).
    /// Bundlebase is read-only, so this returns 0 affected rows for SELECT
    /// and errors for actual DML statements.
    async fn do_put_prepared_statement_update(
        &self,
        cmd: CommandPreparedStatementUpdate,
        _request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
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

        // Check if this is actually a SELECT query (read-only)
        let trimmed = sql.trim().to_uppercase();
        if trimmed.starts_with("SELECT") || trimmed.starts_with("WITH") {
            // For SELECT statements, return 0 rows affected
            // The client should use do_get_prepared_statement instead
            info!("Prepared statement update called for SELECT: {}", handle);
            Ok(0)
        } else {
            // Actual DML is not supported
            Err(Status::unimplemented(
                "Bundlebase is read-only; INSERT/UPDATE/DELETE are not supported",
            ))
        }
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
        metadata::get_flight_info_sql_info(cmd, request)
    }

    /// Get SQL info data.
    async fn do_get_sql_info(
        &self,
        _cmd: CommandGetSqlInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_sql_info()
    }

    /// Get catalogs info.
    async fn get_flight_info_catalogs(
        &self,
        cmd: CommandGetCatalogs,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_catalogs(cmd, request)
    }

    /// Get catalogs data.
    async fn do_get_catalogs(
        &self,
        _cmd: CommandGetCatalogs,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_catalogs()
    }

    /// Get schemas info.
    async fn get_flight_info_schemas(
        &self,
        cmd: CommandGetDbSchemas,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_schemas(cmd, request)
    }

    /// Get schemas data.
    async fn do_get_schemas(
        &self,
        _cmd: CommandGetDbSchemas,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_schemas()
    }

    /// Get tables info.
    async fn get_flight_info_tables(
        &self,
        cmd: CommandGetTables,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_tables(cmd, request)
    }

    /// Get tables data.
    async fn do_get_tables(
        &self,
        _cmd: CommandGetTables,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_tables()
    }

    /// Get table types info.
    async fn get_flight_info_table_types(
        &self,
        cmd: CommandGetTableTypes,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_table_types(cmd, request)
    }

    /// Get table types data.
    async fn do_get_table_types(
        &self,
        _cmd: CommandGetTableTypes,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_table_types()
    }

    /// Get primary keys info (stub).
    async fn get_flight_info_primary_keys(
        &self,
        cmd: CommandGetPrimaryKeys,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_primary_keys(cmd, request)
    }

    /// Get primary keys data (returns empty - no primary keys defined).
    async fn do_get_primary_keys(
        &self,
        _cmd: CommandGetPrimaryKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_primary_keys()
    }

    /// Register SQL info (not needed for read-only server).
    async fn register_sql_info(&self, _id: i32, _result: &SqlInfo) {}
}

/// Encode a schema to IPC format bytes for Flight SQL.
fn encode_schema(schema: &SchemaRef) -> Result<Bytes, Status> {
    // Use FlightInfo's schema encoding which produces the correct IPC format
    let flight_info = FlightInfo::new()
        .try_with_schema(schema)
        .map_err(|e| Status::internal(format!("Failed to encode schema: {}", e)))?;

    Ok(Bytes::from(flight_info.schema))
}

// Re-export for backward compatibility
pub use BundlebaseFlightSqlService as BundlebaseFlightService;
