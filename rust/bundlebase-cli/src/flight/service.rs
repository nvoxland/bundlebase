//! Core Flight SQL service implementation.
//!
//! This service implements session-based state isolation: each authenticated session
//! gets its own `BundleState` instance, ensuring that operations in one session don't
//! affect other sessions. Authentication is required for all requests.

use super::metadata;
use super::prepared_statements::PreparedStatement;
use crate::auth::BundlebaseAuthenticator;
use arrow::datatypes::{Schema, SchemaRef};
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_server::FlightService;
use arrow_flight::sql::server::FlightSqlService;
use arrow_flight::sql::server::PeekableFlightDataStream;
use arrow_flight::sql::{
    ActionClosePreparedStatementRequest, ActionCreatePreparedStatementRequest,
    ActionCreatePreparedStatementResult, Any, CommandGetCatalogs, CommandGetCrossReference,
    CommandGetDbSchemas, CommandGetExportedKeys, CommandGetImportedKeys, CommandGetPrimaryKeys,
    CommandGetSqlInfo, CommandGetTableTypes, CommandGetTables, CommandGetXdbcTypeInfo,
    CommandPreparedStatementQuery, CommandPreparedStatementUpdate, CommandStatementQuery,
    ProstMessageExt, SqlInfo, TicketStatementQuery,
};
use arrow_flight::FlightData;
use arrow_flight::{
    FlightDescriptor, FlightEndpoint, FlightInfo, HandshakeRequest, HandshakeResponse, Ticket,
};
use base64::prelude::*;
use bundlebase::{Bundle, BundleFacade, PassedBundleConfig};
use bundlebase_command::BundleFacadeCommandExt;
use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt};
use parking_lot::RwLock;
use prost::Message;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, info};
use uuid::Uuid;

/// Stream type for FlightData responses.
pub type DoGetStream = Pin<Box<dyn Stream<Item = Result<FlightData, Status>> + Send>>;

/// Redact a live session token for logging.
///
/// A session token grants full access, so it must never appear in logs where it
/// could be harvested and replayed to hijack the session. This keeps a short,
/// non-reconstructable prefix so related log lines can still be correlated.
fn redact_token(token: &str) -> String {
    let shown: String = token.chars().take(12).collect();
    format!("{shown}…<redacted>")
}

/// Session data including state and prepared statements.
struct Session {
    bundle: Arc<dyn BundleFacade>,
    prepared_statements: HashMap<String, PreparedStatement>,
    /// Cached bundle schema (updated when bundle changes).
    bundle_schema: Option<Arc<Schema>>,
    /// Last time this session was accessed (for idle expiration).
    last_accessed: std::time::Instant,
}

/// Store for sessions, keyed by auth token.
type SessionStore = Arc<RwLock<HashMap<String, Session>>>;

/// Maximum idle time before a session is expired (30 minutes).
const SESSION_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Interval between session cleanup sweeps (5 minutes).
const SESSION_CLEANUP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// How long an idle-expired token is remembered so its next request can be
/// told "session expired" rather than the generic "authentication required".
const EXPIRED_TOKEN_RETENTION: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Upper bound on remembered expired tokens; the oldest are forgotten first.
const MAX_EXPIRED_TOKENS: usize = 10_000;

/// Tokens whose sessions were idle-expired, with the time they expired.
/// Bounded by `EXPIRED_TOKEN_RETENTION` and `MAX_EXPIRED_TOKENS`.
type ExpiredTokens = Arc<RwLock<HashMap<String, std::time::Instant>>>;

/// Create a new session store.
fn new_session_store() -> SessionStore {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Remove sessions idle longer than `idle_timeout`, remembering their tokens
/// in `expired_tokens` so the next request with one gets an explicit "session
/// expired" error. Returns how many sessions were removed.
fn sweep_idle_sessions(
    sessions: &SessionStore,
    expired_tokens: &RwLock<HashMap<String, std::time::Instant>>,
    idle_timeout: std::time::Duration,
) -> usize {
    let expired: Vec<String> = {
        let sessions = sessions.read();
        sessions
            .iter()
            .filter(|(_, session)| session.last_accessed.elapsed() > idle_timeout)
            .map(|(token, _)| token.clone())
            .collect()
    };
    {
        let mut sessions = sessions.write();
        for token in &expired {
            sessions.remove(token);
        }
    }
    let now = std::time::Instant::now();
    remember_expired_tokens(
        &mut expired_tokens.write(),
        expired.iter().cloned(),
        now,
        EXPIRED_TOKEN_RETENTION,
        MAX_EXPIRED_TOKENS,
    );
    if !expired.is_empty() {
        info!("Cleaned up {} idle session(s)", expired.len());
    }
    expired.len()
}

/// Record newly expired tokens, then prune entries older than `retention`
/// and, if still over `max`, the oldest ones — so the set stays bounded.
fn remember_expired_tokens(
    expired_tokens: &mut HashMap<String, std::time::Instant>,
    newly_expired: impl Iterator<Item = String>,
    now: std::time::Instant,
    retention: std::time::Duration,
    max: usize,
) {
    for token in newly_expired {
        expired_tokens.insert(token, now);
    }
    expired_tokens.retain(|_, at| now.saturating_duration_since(*at) <= retention);
    if expired_tokens.len() > max {
        let mut by_age: Vec<(std::time::Instant, String)> = expired_tokens
            .iter()
            .map(|(token, at)| (*at, token.clone()))
            .collect();
        by_age.sort_unstable_by_key(|(at, _)| *at);
        let excess = expired_tokens.len() - max;
        for (_, token) in by_age.into_iter().take(excess) {
            expired_tokens.remove(&token);
        }
    }
}

/// The bundlebase Flight SQL service implementation.
///
/// This service supports session-based state isolation. Each authenticated session
/// (identified by auth token) gets its own `BundleState` instance and prepared statement
/// storage. This ensures that concurrent connections from different clients don't
/// interfere with each other.
///
/// # Session Lifecycle
///
/// 1. Client calls `do_handshake` with credentials
/// 2. Server validates credentials and creates a new `BundleState` for the session
/// 3. Server returns an auth token in the handshake response
/// 4. Client includes the token in the `authorization` header of subsequent requests
/// 5. Server looks up the session state by token for each request
///
/// # Authentication Required
///
/// All requests must include a valid auth token. Requests without authentication
/// will receive an `unauthenticated` error.
pub struct BundlebaseFlightSqlService {
    /// Bundle path for creating new session states
    bundle_path: String,
    /// Bundle config for creating new session states
    bundle_config: Option<PassedBundleConfig>,
    /// Whether to open bundles in read-only mode
    read_only: bool,
    /// Session states keyed by auth token
    sessions: SessionStore,
    /// Tokens whose sessions idle-expired (bounded, TTL'd), so a returning
    /// client is told to re-handshake instead of getting a generic auth error.
    expired_tokens: ExpiredTokens,
    authenticator: BundlebaseAuthenticator,
}

impl BundlebaseFlightSqlService {
    /// Create a new Flight SQL service.
    ///
    /// # Arguments
    ///
    /// * `bundle_path` - Path to the bundle (URL or filesystem path)
    /// * `bundle_config` - Optional bundle configuration
    /// * `read_only` - If true, sessions will be read-only (only SELECT/EXPLAIN allowed)
    /// * `authenticator` - Authenticator for validating credentials
    pub fn new(
        bundle_path: String,
        bundle_config: Option<PassedBundleConfig>,
        read_only: bool,
        authenticator: BundlebaseAuthenticator,
    ) -> Self {
        let sessions = new_session_store();
        let expired_tokens: ExpiredTokens = Arc::new(RwLock::new(HashMap::new()));

        // Start background task to clean up idle sessions. Expired tokens are
        // remembered (bounded by count and age) so the client gets an explicit
        // "session expired" error on its next request (bb-8gq.4).
        let cleanup_sessions = Arc::clone(&sessions);
        let cleanup_tokens = Arc::clone(&expired_tokens);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SESSION_CLEANUP_INTERVAL).await;
                sweep_idle_sessions(&cleanup_sessions, &cleanup_tokens, SESSION_IDLE_TIMEOUT);
            }
        });

        Self {
            bundle_path,
            bundle_config,
            read_only,
            sessions,
            expired_tokens,
            authenticator,
        }
    }

    /// Get the session state for a request.
    ///
    /// Looks up the session by auth token from request metadata. A token
    /// whose session has idle-expired is rejected with an explicit
    /// unauthenticated error (the client must re-handshake) — session state
    /// is never silently replaced.
    async fn get_bundle<T>(&self, request: &Request<T>) -> Result<Arc<dyn BundleFacade>, Status> {
        // Try to extract auth token from request metadata
        if let Some(auth_header) = request.metadata().get("authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                // Token format: "Bearer <token>" or just "<token>"
                let token = auth_str.strip_prefix("Bearer ").unwrap_or(auth_str);

                // Look up session state by token
                {
                    let mut sessions = self.sessions.write();
                    if let Some(session) = sessions.get_mut(token) {
                        session.last_accessed = std::time::Instant::now();
                        return Ok(Arc::clone(&session.bundle));
                    }
                }

                // The token's session was idle-expired by the cleanup sweep. Silently handing out a FRESH session
                // here discarded the client's staged work with no signal and
                // raced concurrent requests through create_session
                // (bb-8gq.4). Tell the client to re-handshake instead.
                if self.expired_tokens.write().remove(token).is_some() {
                    info!(
                        "Rejecting expired session token {} — client must re-handshake",
                        redact_token(token)
                    );
                    return Err(Status::unauthenticated(format!(
                        "Session expired after {}s idle; uncommitted session state was \
                         discarded. Call do_handshake to start a new session.",
                        SESSION_IDLE_TIMEOUT.as_secs()
                    )));
                }
            }
        }

        Err(Status::unauthenticated(
            "Authentication required. Call do_handshake first.",
        ))
    }

    /// Get the token from a request's authorization header.
    fn get_token<T>(&self, request: &Request<T>) -> Result<String, Status> {
        let auth_header = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Missing authorization header"))?;

        let auth_str = auth_header
            .to_str()
            .map_err(|_| Status::unauthenticated("Invalid authorization header encoding"))?;

        Ok(auth_str
            .strip_prefix("Bearer ")
            .unwrap_or(auth_str)
            .to_string())
    }

    /// Create a new session for an authenticated user.
    ///
    /// Creates a new bundle facade and prepared statement storage for the session.
    async fn create_session(&self) -> Result<Session, Status> {
        let state: Arc<dyn BundleFacade> = if self.read_only {
            Bundle::open(&self.bundle_path, self.bundle_config.clone())
                .await
                .map_err(|e| Status::internal(format!("Failed to open bundle: {}", e)))?
        } else {
            Bundle::open(&self.bundle_path, self.bundle_config.clone())
                .await
                .map_err(|e| Status::internal(format!("Failed to open bundle: {}", e)))?
                .extend(None)
                .await
                .map_err(|e| Status::internal(format!("Failed to extend bundle: {}", e)))?
        };

        // Try to get initial bundle schema (may be empty for new bundles).
        // described_schema carries column descriptions as Arrow field metadata,
        // so Flight/BI clients see the semantic layer, not just MCP agents.
        let bundle_schema = state
            .described_schema()
            .await
            .ok()
            .map(|s| (*s).clone().into());

        Ok(Session {
            bundle: state,
            prepared_statements: HashMap::new(),
            bundle_schema,
            last_accessed: std::time::Instant::now(),
        })
    }

    /// Get the schema for a SQL query by planning it.
    async fn get_query_schema(
        &self,
        bundle: &Arc<dyn BundleFacade>,
        sql: &str,
    ) -> Result<SchemaRef, Status> {
        let (schema, _shape) = bundle
            .response_schema(sql)
            .await
            .map_err(|e| Status::internal(format!("Failed to get schema: {}", e)))?;
        Ok(schema)
    }

    /// Get the cached bundle schema for a token (if available).
    fn get_bundle_schema_for_token(&self, token: &str) -> Option<Arc<Schema>> {
        self.sessions
            .read()
            .get(token)
            .and_then(|session| session.bundle_schema.clone())
    }

    /// Refresh the cached bundle schema for a session.
    /// Called after mutations (ATTACH, etc.) that may change the schema.
    async fn refresh_schema_cache(&self, token: &str, bundle: &Arc<dyn BundleFacade>) {
        if let Ok(schema) = bundle.described_schema().await {
            if let Some(session) = self.sessions.write().get_mut(token) {
                session.bundle_schema = Some((*schema).clone().into());
            }
        }
    }

    /// Check if there are any active sessions (for testing).
    #[cfg(test)]
    pub fn has_sessions(&self) -> bool {
        !self.sessions.read().is_empty()
    }

    /// Run one idle-session sweep with the given timeout (the background
    /// cleanup task does the same with `SESSION_IDLE_TIMEOUT`).
    #[cfg(test)]
    fn sweep_idle_sessions(&self, idle_timeout: std::time::Duration) -> usize {
        sweep_idle_sessions(&self.sessions, &self.expired_tokens, idle_timeout)
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
        // Parse credentials from authorization header (Basic auth format)
        // Must extract header before consuming the request
        let authorization = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Authorization header not present"))?
            .to_str()
            .map_err(|e| Status::unauthenticated(format!("Authorization not parsable: {}", e)))?
            .to_string();

        let mut request_stream = request.into_inner();

        // Expect "Basic <base64>" format
        let basic_prefix = "Basic ";
        if !authorization.starts_with(basic_prefix) {
            return Err(Status::unauthenticated(format!(
                "Unsupported auth type: {}",
                authorization.split_whitespace().next().unwrap_or("unknown")
            )));
        }

        // Decode base64 credentials
        let base64_creds = &authorization[basic_prefix.len()..];
        let decoded_bytes = BASE64_STANDARD
            .decode(base64_creds)
            .map_err(|e| Status::unauthenticated(format!("Invalid base64 encoding: {}", e)))?;

        let creds_str = String::from_utf8(decoded_bytes)
            .map_err(|_| Status::unauthenticated("Invalid UTF-8 in credentials"))?;

        // Parse username:password
        let parts: Vec<&str> = creds_str.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(Status::unauthenticated(
                "Invalid auth format, expected username:password",
            ));
        }

        let username = parts[0];
        let password = parts[1];

        // Drain any HandshakeRequest body the client sent. PyArrow's
        // `authenticate_basic_token` only sends the Authorization header
        // (no body) — requiring a body broke that and every other
        // header-only client. Treat the body as advisory: we already
        // have the credentials from the header.
        let _ = request_stream.next().await;

        // Validate credentials
        if !self.authenticator.validate(username, password) {
            return Err(Status::unauthenticated("Invalid credentials"));
        }

        // Create a new session for this authenticated connection
        let session = self.create_session().await?;

        // Generate a unique token for this session
        let token = format!("token-{}", Uuid::new_v4());

        // Store the session
        self.sessions.write().insert(token.clone(), session);

        info!(
            "Created new session for user '{}': {}",
            username,
            redact_token(&token)
        );

        // Return success response with the token in both payload and header
        // Flight SQL JDBC driver expects the token in the payload
        let handshake_response = HandshakeResponse {
            protocol_version: 1,
            payload: Bytes::from(format!("Bearer {}", token)),
        };

        let stream: Pin<Box<dyn Stream<Item = Result<HandshakeResponse, Status>> + Send>> =
            Box::pin(futures::stream::once(async { Ok(handshake_response) }));
        let mut response = Response::new(stream);

        // Also set the authorization header for clients that expect it there
        response.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", token)
                .parse()
                .map_err(|_| Status::internal("Failed to set authorization header"))?,
        );

        Ok(response)
    }

    /// Create a prepared statement.
    async fn do_action_create_prepared_statement(
        &self,
        query: ActionCreatePreparedStatementRequest,
        request: Request<arrow_flight::Action>,
    ) -> Result<ActionCreatePreparedStatementResult, Status> {
        let sql = query.query.clone();
        let handle = Uuid::new_v4().to_string();

        // SQL can embed credentials (e.g. ATTACH/SET CONFIG), so log it only at
        // debug, not at the default info level.
        debug!("Creating prepared statement: {} -> {}", handle, sql);

        // Get token and session state for this request
        let token = self.get_token(&request)?;
        let bundle = self.get_bundle(&request).await?;

        // Get schema by planning query (not executing)
        let schema = self.get_query_schema(&bundle, &sql).await?;

        // Store prepared statement in the session
        {
            let mut sessions = self.sessions.write();
            let session = sessions.get_mut(&token).ok_or_else(|| {
                Status::unauthenticated("Session not found. Call do_handshake first.")
            })?;
            session.prepared_statements.insert(
                handle.clone(),
                PreparedStatement {
                    sql,
                    schema: schema.clone(),
                },
            );
        }

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
        request: Request<arrow_flight::Action>,
    ) -> Result<(), Status> {
        let handle = String::from_utf8(query.prepared_statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid prepared statement handle"))?;

        info!("Closing prepared statement: {}", handle);

        let token = self.get_token(&request)?;

        let mut sessions = self.sessions.write();
        let session = sessions.get_mut(&token).ok_or_else(|| {
            Status::unauthenticated("Session not found. Call do_handshake first.")
        })?;
        session.prepared_statements.remove(&handle);
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

        let token = self.get_token(&request)?;

        let schema = {
            let sessions = self.sessions.read();
            let session = sessions.get(&token).ok_or_else(|| {
                Status::unauthenticated("Session not found. Call do_handshake first.")
            })?;
            session
                .prepared_statements
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

        let token = self.get_token(&request)?;

        let (sql, bundle) = {
            let sessions = self.sessions.read();
            let session = sessions.get(&token).ok_or_else(|| {
                Status::unauthenticated("Session not found. Call do_handshake first.")
            })?;
            let sql = session
                .prepared_statements
                .get(&handle)
                .ok_or_else(|| Status::not_found("Prepared statement not found"))?
                .sql
                .clone();
            let state = Arc::clone(&session.bundle);
            (sql, state)
        };

        debug!("Executing prepared statement: {} -> {}", handle, sql);

        let record_stream = bundle
            .execute(&sql, vec![])
            .await
            .map_err(|e| Status::internal(format!("Failed to execute: {}", e)))?;

        // Refresh schema cache after bundlebase commands that might modify schema
        if bundlebase_command::parser::is_command_statement(&sql) {
            self.refresh_schema_cache(&token, &bundle).await;
        }

        // Convert to FlightData stream
        let schema = record_stream.schema();
        let batch_stream =
            record_stream.map(|result| result.map_err(|e| FlightError::ExternalError(Box::new(e))));
        let flight_stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(batch_stream)
            .map_err(|e| Status::from_error(Box::new(e)));

        Ok(Response::new(Box::pin(flight_stream) as DoGetStream))
    }

    /// Execute a prepared statement update (DML).
    /// Bundlebase is read-only, so this returns 0 affected rows for SELECT
    /// and errors for actual DML statements.
    async fn do_put_prepared_statement_update(
        &self,
        cmd: CommandPreparedStatementUpdate,
        request: Request<PeekableFlightDataStream>,
    ) -> Result<i64, Status> {
        let handle = String::from_utf8(cmd.prepared_statement_handle.to_vec())
            .map_err(|_| Status::invalid_argument("Invalid prepared statement handle"))?;

        let token = self.get_token(&request)?;

        let sql = {
            let sessions = self.sessions.read();
            let session = sessions.get(&token).ok_or_else(|| {
                Status::unauthenticated("Session not found. Call do_handshake first.")
            })?;
            session
                .prepared_statements
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

        debug!("Getting flight info for statement: {}", sql);

        // Get session state for this request
        let bundle = self.get_bundle(&request).await?;

        // Get schema by planning the query
        let schema = self.get_query_schema(&bundle, &sql).await?;

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

        debug!("Executing direct statement: {}", sql);

        // Get token and session state for this request
        let token = self.get_token(&request)?;
        let bundle = self.get_bundle(&request).await?;

        // Execute directly via BundleFacade
        let record_stream = bundle
            .execute(&sql, vec![])
            .await
            .map_err(|e| Status::internal(format!("Failed to execute: {}", e)))?;

        // Refresh schema cache after bundlebase commands that might modify schema
        if bundlebase_command::parser::is_command_statement(&sql) {
            self.refresh_schema_cache(&token, &bundle).await;
        }

        // Convert to FlightData stream
        let schema = record_stream.schema();
        let batch_stream =
            record_stream.map(|result| result.map_err(|e| FlightError::ExternalError(Box::new(e))));
        let flight_stream = FlightDataEncoderBuilder::new()
            .with_schema(schema)
            .build(batch_stream)
            .map_err(|e| Status::from_error(Box::new(e)));

        Ok(Response::new(Box::pin(flight_stream) as DoGetStream))
    }

    /// Fallback handler for unknown ticket types.
    async fn do_get_fallback(
        &self,
        _request: Request<Ticket>,
        _message: Any,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
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
        request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        let function_namespaces = self
            .get_token(&request)
            .ok()
            .and_then(|token| {
                self.sessions
                    .read()
                    .get(&token)
                    .map(|session| session.bundle.function_registry().read().namespaces())
            })
            .unwrap_or_default();
        metadata::do_get_schemas(&function_namespaces)
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
        cmd: CommandGetTables,
        request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        // Try to get cached bundle schema and function entries from session
        let token = self.get_token(&request).ok();
        let bundle_schema = token
            .as_ref()
            .and_then(|t| self.get_bundle_schema_for_token(t));
        let function_entries = token
            .as_ref()
            .and_then(|t| {
                self.sessions
                    .read()
                    .get(t.as_str())
                    .map(|session| session.bundle.function_registry().read().names())
            })
            .unwrap_or_default();
        metadata::do_get_tables(cmd, bundle_schema, &function_entries)
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

    /// Get exported keys info.
    async fn get_flight_info_exported_keys(
        &self,
        cmd: CommandGetExportedKeys,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_exported_keys(cmd, request)
    }

    /// Get exported keys data (returns empty - no foreign key constraints).
    async fn do_get_exported_keys(
        &self,
        _cmd: CommandGetExportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_exported_keys()
    }

    /// Get imported keys info.
    async fn get_flight_info_imported_keys(
        &self,
        cmd: CommandGetImportedKeys,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_imported_keys(cmd, request)
    }

    /// Get imported keys data (returns empty - no foreign key constraints).
    async fn do_get_imported_keys(
        &self,
        _cmd: CommandGetImportedKeys,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_imported_keys()
    }

    /// Get cross reference info.
    async fn get_flight_info_cross_reference(
        &self,
        cmd: CommandGetCrossReference,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_cross_reference(cmd, request)
    }

    /// Get cross reference data (returns empty - no foreign key constraints).
    async fn do_get_cross_reference(
        &self,
        _cmd: CommandGetCrossReference,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_cross_reference()
    }

    /// Get XDBC type info.
    async fn get_flight_info_xdbc_type_info(
        &self,
        cmd: CommandGetXdbcTypeInfo,
        request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        metadata::get_flight_info_xdbc_type_info(cmd, request)
    }

    /// Get XDBC type info data.
    async fn do_get_xdbc_type_info(
        &self,
        _cmd: CommandGetXdbcTypeInfo,
        _request: Request<Ticket>,
    ) -> Result<Response<<Self as FlightService>::DoGetStream>, Status> {
        metadata::do_get_xdbc_type_info()
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

    Ok(flight_info.schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remembered_expired_tokens_are_bounded() {
        let now = std::time::Instant::now();
        let retention = std::time::Duration::from_secs(60);
        let mut tokens = HashMap::new();

        // Count bound: oldest are dropped first.
        remember_expired_tokens(
            &mut tokens,
            (0..5).map(|i| format!("t{i}")),
            now,
            retention,
            3,
        );
        assert_eq!(tokens.len(), 3);

        // Age bound: entries older than retention are pruned.
        let later = now + std::time::Duration::from_secs(61);
        remember_expired_tokens(
            &mut tokens,
            std::iter::once("fresh".to_string()),
            later,
            retention,
            3,
        );
        assert_eq!(tokens.len(), 1);
        assert!(tokens.contains_key("fresh"));
    }

    #[tokio::test]
    async fn test_session_expired_by_cleanup_sweep_reports_expiry() {
        // Go through the real expiry path: an issued token whose session is
        // removed by the idle sweep must get the "Session expired" message on
        // its next request, not the generic "Authentication required".
        let svc = BundlebaseFlightSqlService::new(
            "memory:///sweep_expiry_test".to_string(),
            None,
            true,
            crate::auth::BundlebaseAuthenticator::default(),
        );
        let bundle: Arc<dyn BundleFacade> =
            bundlebase::BundleBuilder::create("memory:///sweep_expiry_test", None)
                .await
                .expect("create bundle");
        let token = "token-sweep".to_string();
        svc.sessions.write().insert(
            token.clone(),
            Session {
                bundle,
                prepared_statements: HashMap::new(),
                bundle_schema: None,
                last_accessed: std::time::Instant::now(),
            },
        );

        let mut req = tonic::Request::new(());
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("meta"),
        );
        assert!(svc.get_bundle(&req).await.is_ok(), "live session works");

        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(
            svc.sweep_idle_sessions(std::time::Duration::from_millis(1)),
            1
        );
        assert!(!svc.has_sessions());

        let err = svc
            .get_bundle(&req)
            .await
            .err()
            .expect("expired token must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert!(
            err.message().contains("Session expired"),
            "expected the session-expired message, got: {}",
            err.message()
        );

        // Reported once, then forgotten.
        let err = svc.get_bundle(&req).await.err().expect("still rejected");
        assert!(!err.message().contains("Session expired"));
    }

    #[test]
    fn test_redact_token_does_not_expose_full_token() {
        // A real token is `token-<uuid>`; the uuid is the secret that grants
        // session access. Redaction must not reveal enough to reconstruct it.
        let token = format!("token-{}", Uuid::new_v4());
        let redacted = redact_token(&token);
        assert!(
            !redacted.contains(&token),
            "redacted form must not contain the full token: {redacted}"
        );
        // The uuid body (after the `token-` prefix) must not appear in full.
        let uuid_body = token.strip_prefix("token-").expect("token- prefix");
        assert!(
            !redacted.contains(uuid_body),
            "redacted form must not contain the full uuid: {redacted}"
        );
        assert!(
            redacted.contains("redacted"),
            "redacted form should signal that it is truncated: {redacted}"
        );
    }

    #[test]
    fn test_redact_token_is_char_boundary_safe() {
        // Must not panic on multibyte input even though real tokens are ASCII.
        let redacted = redact_token("tökén-你好世界-abcdefghijkl");
        assert!(redacted.ends_with("…<redacted>"));
    }
}
