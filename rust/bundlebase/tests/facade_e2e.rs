use bundlebase::bundle::{BundleBuilder, BundleFacade};
use bundlebase::test_utils::test_datafile;
use bundlebase::Bundle;
use bundlebase_command::{BundleBuilderExt, BundleFacadeCommandExt};
use datafusion::common::ScalarValue;
use futures::StreamExt;

mod common;

fn init() {
    common::init_catalog();
}

// ==================== BundleBuilder Tests ====================

#[tokio::test]
async fn test_builder_execute_sql_query() {
    init();
    let builder = BundleBuilder::create("memory:///test_execute", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Execute a SQL query via execute()
    let mut stream = builder
        .as_ref()
        .execute("SELECT id, first_name FROM bundle LIMIT 5", vec![])
        .await
        .unwrap();

    let mut row_count = 0;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result.unwrap();
        row_count += batch.num_rows();
        // Verify schema has the expected columns
        assert_eq!(batch.num_columns(), 2);
    }
    assert_eq!(row_count, 5);
}

#[tokio::test]
async fn test_builder_execute_explain_command() {
    init();
    let builder = BundleBuilder::create("memory:///test_execute_explain", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Execute EXPLAIN command via execute()
    let mut stream = builder
        .as_ref()
        .execute("EXPLAIN", vec![])
        .await
        .unwrap();

    let mut batches = Vec::new();
    while let Some(batch_result) = stream.next().await {
        batches.push(batch_result.unwrap());
    }

    // EXPLAIN should return batches with plan_type and plan columns
    assert!(!batches.is_empty());
    assert_eq!(batches[0].num_columns(), 2); // plan_type + plan columns
    assert!(batches[0].num_rows() > 0);
}

#[tokio::test]
async fn test_builder_execute_mutating_command_succeeds() {
    init();
    let builder = BundleBuilder::create("memory:///test_execute_mutate", None)
        .await
        .unwrap();

    // BundleBuilder can execute mutating commands via execute()
    let result = builder
        .as_ref()
        .execute(&format!("ATTACH '{}'", test_datafile("userdata.parquet")), vec![])
        .await;

    // Should succeed (attach the file)
    assert!(result.is_ok(), "ATTACH should succeed on BundleBuilder: {:?}", result.err());

    // Verify the attachment worked
    let schema = builder.schema().await.unwrap();
    assert!(!schema.fields().is_empty(), "Schema should have fields after attach");
}

#[tokio::test]
async fn test_builder_execute_filter_command_succeeds() {
    init();
    let builder = BundleBuilder::create("memory:///test_execute_filter", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    let initial_rows = builder.num_rows().await.unwrap();

    // BundleBuilder can execute FILTER via execute()
    let result = builder
        .as_ref()
        .execute("FILTER WITH SELECT * FROM bundle WHERE id > 10", vec![])
        .await;

    assert!(result.is_ok(), "FILTER should succeed on BundleBuilder: {:?}", result.err());

    // Verify the filter was applied (fewer rows)
    let filtered_rows = builder.num_rows().await.unwrap();
    assert!(filtered_rows < initial_rows, "FILTER should reduce row count");
}

// ==================== Bundle (Read-Only) Tests ====================

#[tokio::test]
async fn test_bundle_execute_sql_query() {
    init();
    // Create and commit a bundle first
    let builder = BundleBuilder::create("memory:///test_bundle_query", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    builder.commit("Test commit").await.unwrap();
    let bundle_url = builder.url().to_string();

    // Open the committed bundle (read-only)
    let bundle = Bundle::open(&bundle_url, None).await.unwrap();

    // Execute a SQL query via execute()
    let mut stream = bundle
        .as_ref()
        .execute("SELECT COUNT(*) as cnt FROM bundle", vec![])
        .await
        .unwrap();

    let mut batches = Vec::new();
    while let Some(batch_result) = stream.next().await {
        batches.push(batch_result.unwrap());
    }

    assert!(!batches.is_empty());
    // COUNT(*) should return one row
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 1);
}

#[tokio::test]
async fn test_bundle_execute_explain_command() {
    init();
    // Create and commit a bundle first
    let builder = BundleBuilder::create("memory:///test_bundle_explain", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    builder.commit("Test commit").await.unwrap();
    let bundle_url = builder.url().to_string();

    // Open the committed bundle (read-only)
    let bundle = Bundle::open(&bundle_url, None).await.unwrap();

    // Execute EXPLAIN command via execute()
    let mut stream = bundle
        .as_ref()
        .execute("EXPLAIN", vec![])
        .await
        .unwrap();

    let mut batches = Vec::new();
    while let Some(batch_result) = stream.next().await {
        batches.push(batch_result.unwrap());
    }

    // EXPLAIN should return batches with plan_type and plan columns
    assert!(!batches.is_empty());
    assert_eq!(batches[0].num_columns(), 2); // plan_type + plan columns
    assert!(batches[0].num_rows() > 0);
}

#[tokio::test]
async fn test_bundle_execute_mutating_command_fails() {
    init();
    // Create and commit a bundle first
    let builder = BundleBuilder::create("memory:///test_bundle_mutate", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    builder.commit("Test commit").await.unwrap();
    let bundle_url = builder.url().to_string();

    // Open the committed bundle (read-only)
    let bundle = Bundle::open(&bundle_url, None).await.unwrap();

    // Attempting to execute a mutating command should fail
    let result = bundle
        .as_ref()
        .execute("ATTACH 'another_file.parquet'", vec![])
        .await;

    match result {
        Ok(_) => panic!("Expected error for mutating command on read-only bundle"),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("Cannot execute 'ATTACH' on read-only bundle"),
                "Expected error about mutating command, got: {}",
                err_msg
            );
        }
    }
}

#[tokio::test]
async fn test_bundle_execute_commit_command_fails() {
    init();
    // Create and commit a bundle first
    let builder = BundleBuilder::create("memory:///test_bundle_commit", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    builder.commit("Test commit").await.unwrap();
    let bundle_url = builder.url().to_string();

    // Open the committed bundle (read-only)
    let bundle = Bundle::open(&bundle_url, None).await.unwrap();

    // COMMIT is a mutating command, should fail
    let result = bundle
        .as_ref()
        .execute("COMMIT 'Another commit'", vec![])
        .await;

    match result {
        Ok(_) => panic!("Expected error for COMMIT command on read-only bundle"),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("Cannot execute 'COMMIT' on read-only bundle"),
                "Expected error about COMMIT command, got: {}",
                err_msg
            );
        }
    }
}

// ==================== Edge Cases ====================

#[tokio::test]
async fn test_execute_with_params() {
    init();
    let builder = BundleBuilder::create("memory:///test_execute_params", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Execute a parameterized query
    let mut stream = builder
        .as_ref()
        .execute(
            "SELECT * FROM bundle WHERE id = $1",
            vec![ScalarValue::Int64(Some(1))],
        )
        .await
        .unwrap();

    let mut row_count = 0;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result.unwrap();
        row_count += batch.num_rows();
    }
    // Should find exactly one row with id=1
    assert_eq!(row_count, 1);
}

#[tokio::test]
async fn test_execute_empty_bundle() {
    init();
    let builder = BundleBuilder::create("memory:///test_execute_empty", None)
        .await
        .unwrap();

    // Execute on empty bundle should work (returns empty result)
    let mut stream = builder
        .as_ref()
        .execute("SELECT * FROM bundle", vec![])
        .await
        .unwrap();

    let mut row_count = 0;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result.unwrap();
        row_count += batch.num_rows();
    }
    assert_eq!(row_count, 0);
}
