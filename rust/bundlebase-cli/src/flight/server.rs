//! Flight SQL server startup.

use super::service::BundlebaseFlightSqlService;
use crate::state::BundleState;
use arrow_flight::flight_service_server::FlightServiceServer;
use std::net::SocketAddr;
use std::sync::Arc;
use tonic::transport::Server;
use tracing::info;

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
