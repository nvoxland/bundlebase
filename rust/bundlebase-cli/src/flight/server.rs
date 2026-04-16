//! Flight SQL server startup.

use super::service::BundlebaseFlightSqlService;
use crate::auth::BundlebaseAuthenticator;
use arrow_flight::flight_service_server::FlightServiceServer;
use bundlebase::PassedBundleConfig;
use std::net::SocketAddr;
use tonic::transport::Server;
use tracing::{info, warn};

/// Start the Flight SQL server.
///
/// This function starts an Arrow Flight SQL server on the specified address
/// and blocks until the server is shut down.
///
/// # Arguments
///
/// * `bundle_path` - Path to the bundle (URL or filesystem path)
/// * `config` - Optional bundle configuration
/// * `read_only` - If true, open in read-only mode (only SELECT/EXPLAIN allowed)
/// * `addr` - The address to bind to (e.g., "0.0.0.0:50051")
///
/// # Returns
///
/// * `Ok(())` - Server shut down cleanly
/// * `Err(BundlebaseError)` - Server failed to start or encountered an error
pub async fn start(
    bundle_path: &str,
    config: Option<PassedBundleConfig>,
    read_only: bool,
    addr: SocketAddr,
) -> Result<(), bundlebase_common::BundlebaseError> {
    info!(
        "Starting Arrow Flight SQL server on {} ({})",
        addr,
        if read_only { "read-only" } else { "read-write" }
    );

    let authenticator = BundlebaseAuthenticator::default();
    if authenticator.is_using_defaults() {
        warn!("Flight server starting with default credentials (admin/password). Set custom credentials for production use.");
    }

    let flight_service =
        BundlebaseFlightSqlService::new(bundle_path.to_string(), config, read_only, authenticator);

    Server::builder()
        .add_service(FlightServiceServer::new(flight_service))
        .serve(addr)
        .await?;

    Ok(())
}
