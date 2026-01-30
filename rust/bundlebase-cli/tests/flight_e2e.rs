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

/// Execute a SQL query via Flight SQL prepared statement and collect the results.
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

/// Execute a SQL query via direct statement (not prepared statement).
async fn execute_query_direct(
    server: &mut FlightTestServer,
    sql: &str,
) -> Result<Vec<RecordBatch>, String> {
    // Use direct statement workflow (CommandStatementQuery -> TicketStatementQuery)
    let flight_info = server
        .client_mut()
        .execute(sql.to_string(), None)
        .await
        .map_err(|e| format!("Failed to execute statement: {}", e))?;

    // Fetch results from endpoints
    let mut batches = Vec::new();
    for endpoint in flight_info.endpoint {
        if let Some(ticket) = endpoint.ticket {
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

    // Query an empty bundle - should return 0 rows with "no_data" column
    let batches = execute_query(&mut server, "SELECT * FROM bundle")
        .await
        .expect("SELECT from empty bundle should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 0, "Empty bundle should have no rows");
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
    execute_query(&mut server, "FILTER WITH SELECT * FROM bundle WHERE id > 1")
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
    execute_query(&mut server, "FILTER WITH SELECT * FROM bundle WHERE id = 1")
        .await
        .expect("FILTER should succeed");

    // Query should succeed with filtered state persisted in connection
    let batches = execute_query(&mut server, "SELECT * FROM bundle")
        .await
        .expect("SELECT should succeed with filtered state");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 1, "Should have 1 row after filter (Alice only)");
}

// =============================================================================
// Table Alias & DBeaver-style SQL Tests via Flight
// =============================================================================

#[tokio::test]
async fn test_alias_qualified_wildcard_empty_bundle() {
    // Reproduces the "Invalid qualifier t" error observed via IntelliJ/DBeaver.
    // When the server auto-creates a session (e.g., after restart), the bundle
    // is empty. Qualified wildcard on an empty bundle must not fail.
    let mut server = FlightTestServer::start().await;
    // No data attached — empty bundle

    let batches = execute_query(&mut server, "SELECT t.* FROM bundle t")
        .await
        .expect("SELECT t.* FROM bundle t on empty bundle should not error");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 0, "Empty bundle should have no rows");
}

#[tokio::test]
async fn test_star_empty_bundle() {
    // Control test: SELECT * FROM bundle (no alias) on empty bundle
    let mut server = FlightTestServer::start().await;

    let batches = execute_query(&mut server, "SELECT * FROM bundle")
        .await
        .expect("SELECT * FROM bundle on empty bundle should not error");

    // Verify schema has exactly one no_data column (not duplicated)
    if let Some(batch) = batches.first() {
        let schema = batch.schema();
        assert_eq!(schema.fields().len(), 1, "Empty bundle should have exactly 1 column, got: {:?}", schema.fields().iter().map(|f| f.name().as_str()).collect::<Vec<_>>());
        assert_eq!(schema.field(0).name(), "no_data");
    }

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 0, "Empty bundle should have no rows");
}

#[tokio::test]
async fn test_alias_qualified_columns_empty_bundle() {
    // SELECT t.no_data FROM bundle t should work on empty bundle
    let mut server = FlightTestServer::start().await;

    let batches = execute_query(&mut server, "SELECT t.no_data FROM bundle t")
        .await
        .expect("SELECT t.no_data FROM bundle t on empty bundle should not error");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 0, "Empty bundle should have no rows");
}

#[tokio::test]
async fn test_alias_qualified_wildcard_prepared() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    let batches = execute_query(&mut server, "SELECT t.* FROM bundle t")
        .await
        .expect("SELECT t.* FROM bundle t should succeed via prepared statement");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

#[tokio::test]
async fn test_alias_qualified_wildcard_direct() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    let batches = execute_query_direct(&mut server, "SELECT t.* FROM bundle t")
        .await
        .expect("SELECT t.* FROM bundle t should succeed via direct statement");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

#[tokio::test]
async fn test_alias_qualified_columns_prepared() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    let batches = execute_query(&mut server, "SELECT t.id, t.name FROM bundle t")
        .await
        .expect("SELECT t.id, t.name FROM bundle t should succeed via prepared statement");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

#[tokio::test]
async fn test_quoted_alias_qualified_wildcard() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // DBeaver-style double-quoting of identifiers
    let batches = execute_query(
        &mut server,
        r#"SELECT "t".* FROM "bundle" "t""#,
    )
    .await
    .expect(r#"SELECT "t".* FROM "bundle" "t" should succeed"#);

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

#[tokio::test]
async fn test_fully_qualified_table() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Fully qualified catalog.schema.table reference
    let batches = execute_query(
        &mut server,
        r#"SELECT * FROM bundlebase."default".bundle"#,
    )
    .await
    .expect("SELECT * FROM bundlebase.default.bundle should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

#[tokio::test]
async fn test_fully_qualified_table_with_alias() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Full DBeaver combo: fully-qualified table with quoted alias and qualified wildcard
    let batches = execute_query(
        &mut server,
        r#"SELECT "t".* FROM bundlebase."default"."bundle" "t""#,
    )
    .await
    .expect(r#"SELECT "t".* FROM bundlebase."default"."bundle" "t" should succeed"#);

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");
}

// =============================================================================
// Direct Statement Tests (non-prepared statement path)
// =============================================================================

#[tokio::test]
async fn test_direct_select_literal() {
    let mut server = FlightTestServer::start().await;
    let batches = execute_query_direct(&mut server, "SELECT 1 as num")
        .await
        .expect("Direct SELECT 1 should succeed");
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(total_rows >= 1, "Should return at least one row");
}

#[tokio::test]
async fn test_direct_select_after_attach() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();

    // ATTACH via direct statement
    execute_query_direct(&mut server, &data.attach_sql)
        .await
        .expect("Direct ATTACH should succeed");

    // SELECT via direct statement
    let batches = execute_query_direct(&mut server, "SELECT * FROM bundle")
        .await
        .expect("Direct SELECT should succeed after ATTACH");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows (Alice and Bob)");

    // Verify actual columns exist (not just "message")
    assert!(!batches.is_empty(), "Should have at least one batch");
    let schema = batches[0].schema();
    let column_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(column_names.contains(&"id"), "Should have 'id' column");
    assert!(column_names.contains(&"name"), "Should have 'name' column");
}

#[tokio::test]
async fn test_direct_attach_command() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();

    let result = execute_query_direct(&mut server, &data.attach_sql).await;
    assert!(result.is_ok(), "Direct ATTACH should succeed");
}

#[tokio::test]
async fn test_direct_filter_and_select() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::with_content("id,name\n1,Alice\n2,Bob\n3,Carol\n");

    // ATTACH, FILTER, SELECT all via direct statements
    execute_query_direct(&mut server, &data.attach_sql)
        .await
        .expect("Direct ATTACH should succeed");

    execute_query_direct(&mut server, "FILTER WITH SELECT * FROM bundle WHERE id > 1")
        .await
        .expect("Direct FILTER should succeed");

    let batches = execute_query_direct(&mut server, "SELECT * FROM bundle")
        .await
        .expect("Direct SELECT should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 2, "Should have 2 rows after filter");
}

// =============================================================================
// bundle_info Tables Tests via Flight
// =============================================================================

#[tokio::test]
async fn test_bundle_info_status_after_attach() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Query bundle_info.status - should show the uncommitted attach change
    let batches = execute_query(&mut server, "SELECT * FROM bundle_info.status")
        .await
        .expect("SELECT FROM bundle_info.status should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        total_rows >= 1,
        "bundle_info.status should have at least 1 row showing the uncommitted attach change"
    );
}

#[tokio::test]
async fn test_bundle_info_blocks_after_attach() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Query bundle_info.blocks - should show the attached block
    let batches = execute_query(&mut server, "SELECT * FROM bundle_info.blocks")
        .await
        .expect("SELECT FROM bundle_info.blocks should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        total_rows >= 1,
        "bundle_info.blocks should have at least 1 row after attach"
    );
}

#[tokio::test]
async fn test_bundle_info_details_after_attach() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Query bundle_info.details - should show bundle metadata
    let batches = execute_query(&mut server, "SELECT * FROM bundle_info.details")
        .await
        .expect("SELECT FROM bundle_info.details should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        total_rows, 1,
        "bundle_info.details should have exactly 1 row with bundle metadata"
    );

    // Verify that we have actual bundle details, not empty data
    assert!(!batches.is_empty(), "Should have at least one batch");
    let schema = batches[0].schema();
    let column_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        column_names.contains(&"id") || column_names.contains(&"url"),
        "bundle_info.details should have id or url column"
    );
}

#[tokio::test]
async fn test_bundle_info_packs_after_attach() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Query bundle_info.packs - should show at least the base pack
    let batches = execute_query(&mut server, "SELECT * FROM bundle_info.packs")
        .await
        .expect("SELECT FROM bundle_info.packs should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        total_rows >= 1,
        "bundle_info.packs should have at least 1 row (base pack)"
    );
}

#[tokio::test]
async fn test_bundle_info_history_after_commit() {
    let mut server = FlightTestServer::start().await;
    let data = TestData::new();
    data.attach(&mut server).await;

    // Commit the changes
    execute_query(&mut server, "COMMIT 'Test commit for history'")
        .await
        .expect("COMMIT should succeed");

    // Query bundle_info.history - should show the commit
    let batches = execute_query(&mut server, "SELECT * FROM bundle_info.history")
        .await
        .expect("SELECT FROM bundle_info.history should succeed");

    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        total_rows >= 1,
        "bundle_info.history should have at least 1 row after commit"
    );
}

// =============================================================================
// Authentication Failure Tests
// =============================================================================

#[tokio::test]
async fn test_auth_wrong_password() {
    let (_server, mut client) = FlightTestServer::start_unauthenticated().await;

    let result = client.handshake("admin", "wrong_password").await;
    assert!(result.is_err(), "Handshake with wrong password should fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Invalid credentials") || err.contains("UNAUTHENTICATED"),
        "Error should indicate authentication failure, got: {}",
        err
    );
}

#[tokio::test]
async fn test_auth_wrong_username() {
    let (_server, mut client) = FlightTestServer::start_unauthenticated().await;

    let result = client.handshake("unknown_user", "password").await;
    assert!(result.is_err(), "Handshake with wrong username should fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Invalid credentials") || err.contains("UNAUTHENTICATED"),
        "Error should indicate authentication failure, got: {}",
        err
    );
}

#[tokio::test]
async fn test_query_without_auth() {
    let (_server, mut client) = FlightTestServer::start_unauthenticated().await;

    // Try to execute a query without authenticating first
    let result = client.execute("SELECT 1".to_string(), None).await;
    assert!(
        result.is_err(),
        "Query without authentication should fail"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("unauthenticated") || err.contains("Authentication required") || err.contains("UNAUTHENTICATED"),
        "Error should indicate missing auth, got: {}",
        err
    );
}

#[tokio::test]
async fn test_fabricated_token_rejected() {
    let (_server, mut client) = FlightTestServer::start_unauthenticated().await;

    // Manually set a fabricated token that was never issued by the server
    client.set_token("token-00000000-0000-0000-0000-000000000000".to_string());

    let result = client.execute("SELECT 1".to_string(), None).await;
    assert!(
        result.is_err(),
        "Fabricated token should be rejected"
    );
}
