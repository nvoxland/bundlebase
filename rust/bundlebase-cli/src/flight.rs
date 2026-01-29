//! Arrow Flight SQL server implementation for bundlebase.
//!
//! This module provides an Arrow Flight SQL service that allows JDBC clients
//! to connect and execute SQL queries against bundlebase bundles.

mod metadata;
mod prepared_statements;
mod server;
mod service;

// Re-export public API
pub use server::start;
pub use service::BundlebaseFlightSqlService;

#[cfg(test)]
mod tests {
    use crate::auth::BundlebaseAuthenticator;
    use super::service::BundlebaseFlightSqlService;

    #[tokio::test]
    async fn test_flight_sql_service_instantiation() {
        // Service should instantiate successfully with bundle path
        let _service = BundlebaseFlightSqlService::new(
            "memory:///flight_test".to_string(),
            None,
            true,  // create
            false, // read_only
            BundlebaseAuthenticator::default(),
        );
    }

    #[tokio::test]
    async fn test_session_store_lifecycle() {
        let service = BundlebaseFlightSqlService::new(
            "memory:///session_test".to_string(),
            None,
            true,  // create
            false, // read_only
            BundlebaseAuthenticator::default(),
        );

        // Sessions store should start empty
        assert!(!service.has_sessions());
    }
}
