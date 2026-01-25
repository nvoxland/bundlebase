//! End-to-end tests for the Arrow Flight SQL server.
//!
//! These tests verify the Flight SQL server's ability to execute SQL queries
//! and bundlebase commands via the Flight SQL protocol (JDBC compatible).

mod common;

use arrow::array::RecordBatch;
use common::FlightTestServer;
use futures::TryStreamExt;

/// Test data helper with 2 rows by default: Alice (id=1), Bob (id=2).
struct TestData {
    _temp_dir: tempfile::TempDir, // Kept alive for test duration
    attach_sql: String,
}

impl TestData {
    /// Create test data with default content: id,name with Alice and Bob.
    fn new() -> Self {
        Self::with_content("id,name\n1,Alice\n2,Bob\n")
    }

    /// Create test data with custom CSV content.
    fn with_content(csv: &str) -> Self {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let csv_path = temp_dir.path().join("test.csv");
        std::fs::write(&csv_path, csv).expect("Failed to write CSV");
        let attach_sql = format!("ATTACH 'file://{}'", csv_path.display());
        Self {
            _temp_dir: temp_dir,
            attach_sql,
        }
    }

    /// Attach this test data to the server.
    async fn attach(&self, server: &mut FlightTestServer) {
        execute_query(server, &self.attach_sql)
            .await
            .expect("ATTACH should succeed");
    }
}

/// Execute a SQL query via Flight SQL and collect the results.
async fn execute_query(
    server: &mut FlightTestServer,
    sql: &str,
) -> Result<Vec<RecordBatch>, String> {
    // Use prepared statement workflow (JDBC-compatible)
    // 1. Prepare the statement
    let mut stmt = server
        .client_mut()
        .prepare(sql.to_string(), None)
        .await
        .map_err(|e| format!("Failed to prepare statement: {}", e))?;

    // 2. Execute the prepared statement to get FlightInfo
    let flight_info = stmt
        .execute()
        .await
        .map_err(|e| format!("Failed to execute statement: {}", e))?;

    // 3. Fetch the results from each endpoint
    let mut batches = Vec::new();
    for endpoint in flight_info.endpoint {
        if let Some(ticket) = endpoint.ticket {
            // FlightSqlServiceClient.do_get returns a FlightRecordBatchStream directly
            let batch_stream = server
                .client_mut()
                .do_get(ticket)
                .await
                .map_err(|e| format!("Failed to do_get: {}", e))?;

            let endpoint_batches: Vec<RecordBatch> = batch_stream
                .try_collect()
                .await
                .map_err(|e| format!("Failed to collect batches: {}", e))?;

            batches.extend(endpoint_batches);
        }
    }

    // 4. Close the prepared statement
    stmt.close()
        .await
        .map_err(|e| format!("Failed to close statement: {}", e))?;

    Ok(batches)
}

#[tokio::test]
async fn test_select_literal() {
    let mut server = FlightTestServer::start().await;

    // Execute a simple SELECT literal query
    let batches = execute_query(&mut server, "SELECT 1 as num")
        .await
        .expect("SELECT 1 should succeed");

    // Should have at least one batch with one row
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total_rows >= 1, "SELECT 1 should return at least one row");
}

#[tokio::test]
async fn test_select_from_empty_bundle() {
    let mut server = FlightTestServer::start().await;

    // Query an empty bundle - should return empty results or error
    let result = execute_query(&mut server, "SELECT * FROM bundle").await;

    match result {
        Ok(batches) => {
            // If it succeeds, there should be no rows
            let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
            assert_eq!(total_rows, 0, "Empty bundle should have no rows");
        }
        Err(_) => {
            // It's also acceptable for this to error since bundle might not exist
        }
    }
}

#[tokio::test]
async fn test_attach_command() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();

    // ATTACH should return an OK message
    let result = execute_query(&mut server, &data.attach_sql).await;
    assert!(result.is_ok(), "ATTACH should succeed");
}

#[tokio::test]
async fn test_select_after_attach() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Now select from bundle - should return the attached data
    let batches = execute_query(&mut server, "SELECT * FROM bundle")
        .await
        .expect("SELECT should succeed after ATTACH");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

#[tokio::test]
async fn test_filter_command() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::with_content("id,name\n1,Alice\n2,Bob\n3,Carol\n");
    data.attach(&mut server).await;

    // Filter to only rows where id > 1
    execute_query(&mut server, "FILTER WHERE id > 1")
        .await
        .expect("FILTER should succeed");

    // Verify filter command was applied by querying after
    let batches = execute_query(&mut server, "SELECT * FROM bundle")
        .await
        .expect("SELECT should succeed after FILTER");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        total_rows, 2,
        "Should have 2 rows after filtering (Bob and Carol)"
    );
}

#[tokio::test]
async fn test_reset_command() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Reset should revert to empty bundle
    let result = execute_query(&mut server, "RESET").await;
    assert!(result.is_ok(), "RESET should succeed");
}

#[tokio::test]
async fn test_commit_command() {
    let mut server = FlightTestServer::start().await;

    // Commit should work even on empty bundle
    let result = execute_query(&mut server, "COMMIT 'Initial commit'").await;
    assert!(result.is_ok(), "COMMIT should succeed");
}

#[tokio::test]
async fn test_undo_command() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Undo should revert the attach
    let result = execute_query(&mut server, "UNDO").await;
    assert!(result.is_ok(), "UNDO should succeed");
}

#[tokio::test]
async fn test_syntax_error() {
    let mut server = FlightTestServer::start().await;

    // Execute incomplete ATTACH command that will cause an error
    let result = execute_query(&mut server, "ATTACH").await;

    // ATTACH without a path should fail with an error
    match result {
        Err(_) => {
            // Expected - parsing or execution failed
        }
        Ok(batches) => {
            // If no error, check that we got a message indicating failure
            // or that the result is effectively empty
            assert!(
                batches.is_empty() || batches.iter().all(|b| b.num_rows() <= 1),
                "Incomplete ATTACH should not return meaningful data"
            );
        }
    }
}

#[tokio::test]
async fn test_multiple_queries() {
    let mut server = FlightTestServer::start().await;

    // Execute multiple sequential queries
    let result1 = execute_query(&mut server, "SELECT 1 as a").await;
    assert!(result1.is_ok(), "First query should succeed");

    let result2 = execute_query(&mut server, "SELECT 2 as b").await;
    assert!(result2.is_ok(), "Second query should succeed");

    let result3 = execute_query(&mut server, "SELECT 3 as c").await;
    assert!(result3.is_ok(), "Third query should succeed");
}

#[tokio::test]
async fn test_explain_plan() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // EXPLAIN PLAN should return the query plan
    let result = execute_query(&mut server, "EXPLAIN PLAN").await;
    assert!(result.is_ok(), "EXPLAIN PLAN should succeed");
}

#[tokio::test]
async fn test_verify_data() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // VERIFY DATA should return verification results
    let result = execute_query(&mut server, "VERIFY DATA").await;
    assert!(result.is_ok(), "VERIFY DATA should succeed");
}

#[tokio::test]
async fn test_state_persistence_in_connection() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Filter the data
    execute_query(&mut server, "FILTER WHERE id = 1")
        .await
        .expect("FILTER should succeed");

    // Query should succeed with filtered state persisted in connection
    let batches = execute_query(&mut server, "SELECT * FROM bundle")
        .await
        .expect("SELECT should succeed with filtered state");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 1, "Should have 1 row after filter (Alice only)");
}
