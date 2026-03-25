//! Shared test utilities for Flight SQL integration tests.
//!
//! This module provides infrastructure for testing the Arrow Flight SQL server.

/// Ensure the catalog schema provider hook is installed for tests.
pub fn init_catalog() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| { bundlebase_catalog::init(); });
}

use arrow_flight::flight_service_server::FlightServiceServer;
use arrow_flight::sql::client::FlightSqlServiceClient;
use bundlebase::BundleBuilder;
use bundlebase_cli::auth::BundlebaseAuthenticator;
use bundlebase_cli::flight::BundlebaseFlightSqlService as FlightService;
use std::net::{SocketAddr, TcpListener};
use tokio::sync::oneshot;
use tonic::transport::{Channel, Server};

/// Find an available port for the test server.
pub fn get_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to port");
    let addr = listener.local_addr().expect("Failed to get local address");
    addr.port()
}

/// A handle to a running Flight SQL test server.
///
/// When dropped, signals the server to shut down.
pub struct FlightTestServer {
    pub client: FlightSqlServiceClient<Channel>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl FlightTestServer {
    /// Start a new Flight SQL test server with an empty bundle.
    pub async fn start() -> Self {
        init_catalog();
        let bundle_path = format!(
            "memory:///flight_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Pre-create the bundle so the flight service can open it
        let builder = BundleBuilder::create(&bundle_path, None)
            .await
            .expect("Failed to create test bundle");
        builder.commit("Initial commit").await.expect("Failed to commit");

        Self::start_with_bundle_path(&bundle_path).await
    }

    /// Start a new Flight SQL test server with the given bundle path.
    pub async fn start_with_bundle_path(bundle_path: &str) -> Self {
        let port = get_available_port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port)
            .parse()
            .expect("Invalid address");

        let flight_service = FlightService::new(
            bundle_path.to_string(),
            None,
            false, // read_only: false - tests need to modify bundle
            BundlebaseAuthenticator::default(),
        );

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Spawn the server
        let server_addr = addr;
        tokio::spawn(async move {
            Server::builder()
                .add_service(FlightServiceServer::new(flight_service))
                .serve_with_shutdown(server_addr, async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("Flight server failed");
        });

        // Wait for server to be ready by attempting connection with retries
        let channel = {
            let mut attempts = 0;
            let max_attempts = 20;
            loop {
                match Channel::from_shared(format!("http://{}", addr))
                    .expect("Invalid URI")
                    .connect()
                    .await
                {
                    Ok(channel) => break channel,
                    Err(_e) if attempts < max_attempts => {
                        attempts += 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                    Err(e) => panic!(
                        "Failed to connect to Flight SQL server after {} attempts: {}",
                        max_attempts, e
                    ),
                }
            }
        };

        let mut client = FlightSqlServiceClient::new(channel);

        // Authenticate with default credentials (admin:password)
        client
            .handshake("admin", "password")
            .await
            .expect("Handshake should succeed with default credentials");

        Self {
            client,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Start a server without authenticating the client.
    /// Returns the server handle and an unauthenticated client.
    pub async fn start_unauthenticated() -> (Self, FlightSqlServiceClient<Channel>) {
        let bundle_path = format!(
            "memory:///flight_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Pre-create the bundle so the flight service can open it
        let builder = BundleBuilder::create(&bundle_path, None)
            .await
            .expect("Failed to create test bundle");
        builder.commit("Initial commit").await.expect("Failed to commit");

        let port = get_available_port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port)
            .parse()
            .expect("Invalid address");

        let flight_service = FlightService::new(
            bundle_path,
            None,
            false,
            BundlebaseAuthenticator::default(),
        );

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let server_addr = addr;
        tokio::spawn(async move {
            Server::builder()
                .add_service(FlightServiceServer::new(flight_service))
                .serve_with_shutdown(server_addr, async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("Flight server failed");
        });

        // Wait for server to be ready
        let channel = {
            let mut attempts = 0;
            let max_attempts = 20;
            loop {
                match Channel::from_shared(format!("http://{}", addr))
                    .expect("Invalid URI")
                    .connect()
                    .await
                {
                    Ok(channel) => break channel,
                    Err(_e) if attempts < max_attempts => {
                        attempts += 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                    }
                    Err(e) => panic!(
                        "Failed to connect to Flight SQL server after {} attempts: {}",
                        max_attempts, e
                    ),
                }
            }
        };

        // Authenticated client for the server handle
        let mut auth_client = FlightSqlServiceClient::new(channel.clone());
        auth_client
            .handshake("admin", "password")
            .await
            .expect("Handshake should succeed");

        // Unauthenticated client for testing
        let unauth_client = FlightSqlServiceClient::new(channel);

        let server = Self {
            client: auth_client,
            shutdown_tx: Some(shutdown_tx),
        };

        (server, unauth_client)
    }

    /// Get a mutable reference to the Flight SQL client.
    pub fn client_mut(&mut self) -> &mut FlightSqlServiceClient<Channel> {
        &mut self.client
    }
}

impl Drop for FlightTestServer {
    fn drop(&mut self) {
        // Signal the server to shut down
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}
