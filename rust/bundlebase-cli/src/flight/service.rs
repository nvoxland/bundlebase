//! Core Flight SQL service implementation.
//!
//! This service implements session-based state isolation: each authenticated session
//! gets its own `BundleState` instance, ensuring that operations in one session don't
//! affect other sessions. Anonymous requests (without auth token) use a shared default state.

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
use bundlebase::{Bundle, BundleBuilder, BundleConfig};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use parking_lot::RwLock;
use prost::Message;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;
use uuid::Uuid;

/// Store for session states, keyed by auth token.
pub type SessionStore = Arc<RwLock<HashMap<String, Arc<BundleState>>>>;

/// Create a new session store.
pub fn new_session_store() -> SessionStore {
    Arc::new(RwLock::new(HashMap::new()))
}

/// The bundlebase Flight SQL service implementation.
///
/// This service supports session-based state isolation. Each authenticated session
/// (identified by auth token) gets its own `BundleState` instance. This ensures
/// that concurrent connections from different clients don't interfere with each other.
///
/// # Session Lifecycle
///
/// 1. Client calls `do_handshake` with credentials
/// 2. Server validates credentials and creates a new `BundleState` for the session
/// 3. Server returns an auth token in the handshake response
/// 4. Client includes the token in the `authorization` header of subsequent requests
/// 5. Server looks up the session state by token for each request
///
/// # Anonymous Requests
///
/// Requests without an auth token use the default shared state for backward compatibility.
pub struct BundlebaseFlightSqlService {
    /// Default state for anonymous requests (backward compatibility)
    default_state: Arc<BundleState>,
    /// Bundle path for creating new session states
    bundle_path: String,
    /// Bundle config for creating new session states
    bundle_config: Option<BundleConfig>,
    /// Whether to create bundles (vs open existing)
    create_bundle: bool,
    /// Session states keyed by auth token
    sessions: SessionStore,
    authenticator: BundlebaseAuthenticator,
    prepared_statements: PreparedStatementStore,
}

impl BundlebaseFlightSqlService {
    /// Create a new Flight SQL service with the given bundle state.
    ///
    /// This constructor maintains backward compatibility by using the provided state
    /// as both the default state and for deriving the bundle path for new sessions.
    pub fn new(state: Arc<BundleState>) -> Self {
        // Extract bundle path from the state for creating new sessions
        let bundle_path = {
            let guard = state.bundle.read();
            guard.bundle().url().to_string()
        };

        Self {
            default_state: state,
            bundle_path,
            bundle_config: None,
            create_bundle: false,
            sessions: new_session_store(),
            authenticator: BundlebaseAuthenticator::default(),
            prepared_statements: super::prepared_statements::new_store(),
        }
    }

    /// Create a new Flight SQL service with custom authenticator.
    pub fn with_authenticator(
        state: Arc<BundleState>,
        authenticator: BundlebaseAuthenticator,
    ) -> Self {
        let bundle_path = {
            let guard = state.bundle.read();
            guard.bundle().url().to_string()
        };

        Self {
            default_state: state,
            bundle_path,
            bundle_config: None,
            create_bundle: false,
            sessions: new_session_store(),
            authenticator,
            prepared_statements: super::prepared_statements::new_store(),
        }
    }

    /// Get the session state for a request.
    ///
    /// Looks up the session by auth token from request metadata. If no token is found
    /// or the token is invalid, returns the default shared state.
    fn get_session_state<T>(&self, request: &Request<T>) -> Arc<BundleState> {
        // Try to extract auth token from request metadata
        if let Some(auth_header) = request.metadata().get("authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                // Token format: "Bearer <token>" or just "<token>"
                let token = auth_str
                    .strip_prefix("Bearer ")
                    .unwrap_or(auth_str);

                // Look up session state by token
                if let Some(state) = self.sessions.read().get(token) {
                    return Arc::clone(state);
                }
            }
        }

        // Fall back to default state for anonymous requests
        Arc::clone(&self.default_state)
    }

    /// Create a new session state for an authenticated user.
    async fn create_session_state(&self) -> Result<Arc<BundleState>, Status> {
        let builder = if self.create_bundle {
            BundleBuilder::create(&self.bundle_path, self.bundle_config.clone())
                .await
                .map_err(|e| Status::internal(format!("Failed to create bundle: {}", e)))?
        } else {
            Bundle::open(&self.bundle_path, self.bundle_config.clone())
                .await
                .map_err(|e| Status::internal(format!("Failed to open bundle: {}", e)))?
                .extend(None)
                .map_err(|e| Status::internal(format!("Failed to extend bundle: {}", e)))?
        };

        Ok(Arc::new(BundleState::new(builder)))
    }

    /// Get the schema for a SQL query by planning it.
    async fn get_query_schema(&self, state: &Arc<BundleState>, sql: &str) -> Result<SchemaRef, Status> {
        // First check if it's a bundlebase command with known schema
        if let Some(schema) = sql_executor::get_command_schema(sql) {
            return Ok(schema);
        }

        // For standard SQL, we need to plan the query to get the schema
        let builder = {
            let guard = state.bundle.read();
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

    /// Get the sessions store (for testing).
    #[cfg(test)]
    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }
}

#[tonic::async_trait]
impl FlightSqlService for BundlebaseFlightSqlService {
    type FlightService = BundlebaseFlightSqlService;

    /// Handle handshake for authentication.
    ///
    /// On successful authentication, creates a new `BundleState` for the session
    /// and returns an auth token that the client should include in subsequent requests.
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

        // Create a new session state for this authenticated connection
        let session_state = self.create_session_state().await?;

        // Generate a unique token for this session
        let token = format!("token-{}", Uuid::new_v4());

        // Store the session state
        self.sessions.write().insert(token.clone(), session_state);

        info!("Created new session for user '{}': {}", username, token);

        // Return success response with the token
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
        request: Request<arrow_flight::Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        let sql = query.query.clone();
        let handle = Uuid::new_v4().to_string();

        info!("Creating prepared statement: {} -> {}", handle, sql);

        // Get session state for this request
        let state = self.get_session_state(&request);

        // Get schema by planning query (not executing)
        let schema = self.get_query_schema(&state, &sql).await?;

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
        request: Request<Ticket>,
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

        // Get session state for this request
        let state = self.get_session_state(&request);

        execute_query_streaming(&state, sql).await
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

        // Get session state for this request
        let state = self.get_session_state(&request);

        // Get schema by planning the query
        let schema = self.get_query_schema(&state, &sql).await?;

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
        request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // Decode SQL from statement_handle (we stored the SQL bytes directly)
        let sql = String::from_utf8(ticket.statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid statement handle encoding"))?;

        info!("Executing direct statement: {}", sql);

        // Get session state for this request
        let state = self.get_session_state(&request);

        execute_query_streaming(&state, sql).await
    }

    /// Fallback handler for unknown ticket types (backward compatibility).
    async fn do_get_fallback(
        &self,
        request: Request<Ticket>,
        _message: Any,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // Get session state before consuming the request
        let state = self.get_session_state(&request);

        // Try raw SQL in ticket (existing behavior for backward compatibility)
        let ticket = request.into_inner();
        if let Ok(sql) = String::from_utf8(ticket.ticket.to_vec()) {
            info!("Executing fallback query: {}", sql);
            return execute_query_streaming(&state, sql).await;
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
