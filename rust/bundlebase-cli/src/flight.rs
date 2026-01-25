//! Arrow Flight SQL server implementation for bundlebase.
//!
//! This module provides an Arrow Flight SQL service that allows JDBC clients
//! to connect and execute SQL queries against bundlebase bundles.

mod execution;
mod metadata;
mod prepared_statements;
mod server;
mod service;

// Re-export public API
pub use server::start;
pub use service::{BundlebaseFlightService, BundlebaseFlightSqlService};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BundleState;
    use arrow::datatypes::Schema;
    use bundlebase::BundleBuilder;
    use std::sync::Arc;

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
        service.prepared_statements().write().insert(
            handle.clone(),
            prepared_statements::PreparedStatement {
                sql: "SELECT 1".to_string(),
                schema: schema.clone(),
            },
        );

        // Verify it exists
        assert!(service.prepared_statements().read().contains_key(&handle));

        // Remove it
        service.prepared_statements().write().remove(&handle);

        // Verify it's gone
        assert!(!service.prepared_statements().read().contains_key(&handle));
    }
}
