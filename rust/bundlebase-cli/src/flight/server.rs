//! Flight SQL server startup.

use super::service::BundlebaseFlightSqlService;
use crate::auth::BundlebaseAuthenticator;
use arrow_flight::flight_service_server::FlightServiceServer;
use std::net::SocketAddr;
use tonic::transport::Server;
use tracing::info;

/// Start the Flight SQL server.
///
/// This function starts an Arrow Flight SQL server on the specified address
/// and blocks until the server is shut down.
///
/// # Arguments
///
/// * `bundle_path` - Path to the bundle (URL or filesystem path)
/// * `create` - If true, create the bundle; if false, open existing
/// * `addr` - The address to bind to (e.g., "0.0.0.0:50051")
///
/// # Returns
///
/// * `Ok(())` - Server shut down cleanly
/// * `Err(BundlebaseError)` - Server failed to start or encountered an error
pub async fn start(
    bundle_path: &str,
    create: bool,
    addr: SocketAddr,
) -> Result<(), bundlebase::BundlebaseError> {
    info!("Starting Arrow Flight SQL server on {}", addr);

    let flight_service = BundlebaseFlightSqlService::new(
        bundle_path.to_string(),
        None,
        create,
        BundlebaseAuthenticator::default(),
    );

    Server::builder()
        .add_service(FlightServiceServer::new(flight_service))
        .serve(addr)
        .await?;

    Ok(())
}
