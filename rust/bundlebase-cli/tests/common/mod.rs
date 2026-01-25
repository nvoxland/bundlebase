//! Shared test utilities for Flight SQL integration tests.
//!
//! This module provides infrastructure for testing the Arrow Flight SQL server.

use arrow_flight::flight_service_server::FlightServiceServer;
use arrow_flight::sql::client::FlightSqlServiceClient;
use bundlebase::BundleBuilder;
use bundlebase_cli::flight::BundlebaseFlightSqlService as FlightService;
use bundlebase_cli::BundleState;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
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
    pub addr: SocketAddr,
    pub client: FlightSqlServiceClient<Channel>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl FlightTestServer {
    /// Start a new Flight SQL test server with an empty bundle.
    pub async fn start() -> Self {
        let builder = BundleBuilder::create(
            &format!(
                "memory:///flight_test_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("System time before UNIX epoch")
                    .as_nanos()
            ),
            None,
        )
        .await
        .expect("Failed to create test bundle");

        Self::start_with_builder(builder).await
    }

    /// Start a new Flight SQL test server with the given bundle builder.
    pub async fn start_with_builder(builder: BundleBuilder) -> Self {
        let port = get_available_port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port)
            .parse()
            .expect("Invalid address");

        let state = Arc::new(BundleState::new(builder));
        let flight_service = FlightService::new(state);

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

        let client = FlightSqlServiceClient::new(channel);

        Self {
            addr,
            client,
            shutdown_tx: Some(shutdown_tx),
        }
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
