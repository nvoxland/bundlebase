//! E2E tests for file version validation.
//!
//! These tests verify that the version validation system works correctly:
//! - Queries fail when source files have changed since the bundle was created
//! - Queries succeed when source files remain unchanged

use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, random_memory_url};
use bundlebase::Bundle;
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use bundlebase_io::IOReadWriteFile;
use bytes::Bytes;
use futures::TryStreamExt;

mod common;

fn init() {
    common::init_catalog();
}

/// Test that querying a bundle fails when the source CSV file has been modified.
#[tokio::test]
async fn test_query_fails_when_source_file_changed_csv() -> Result<(), BundlebaseError> {
    init();
    // 1. Create a directory for our test data
    let data_dir = random_memory_dir();
    let csv_file = data_dir.writable_file("test_data.csv")?;

    // Create initial CSV content
    let initial_content = "id,name,value\n1,Alice,100\n2,Bob,200\n";
    csv_file.write(Bytes::from(initial_content)).await?;

    // 2. Create a bundle and attach the CSV file
    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(csv_file.url().as_str(), None).await?;

    // Verify initial query works
    let rows_before = builder.num_rows().await?;
    assert_eq!(rows_before, 2, "Initial query should return 2 rows");

    // 3. Commit the bundle
    builder.commit("Initial commit").await?;

    // 4. Modify the CSV file (changes the version)
    let modified_content = "id,name,value\n1,Alice,100\n2,Bob,200\n3,Carol,300\n";
    csv_file.write(Bytes::from(modified_content)).await?;

    // 5. Reload the bundle from disk
    let loaded_bundle = Bundle::open(bundle_url.as_str(), None).await?;

    // 6. Query the bundle - should fail with version mismatch
    let result = loaded_bundle.num_rows().await;

    // 7. Verify the error
    assert!(
        result.is_err(),
        "Query should fail when source file has changed"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Version mismatch") || err_msg.contains("version mismatch"),
        "Error should mention version mismatch. Got: {}",
        err_msg
    );
    assert!(
        err_msg.contains("source file has changed") || err_msg.contains("has changed"),
        "Error should explain the file has changed. Got: {}",
        err_msg
    );

    Ok(())
}

/// Test that querying a bundle succeeds when the source file remains unchanged.
#[tokio::test]
async fn test_query_succeeds_when_source_unchanged() -> Result<(), BundlebaseError> {
    init();
    // 1. Create a directory for our test data
    let data_dir = random_memory_dir();
    let csv_file = data_dir.writable_file("test_data.csv")?;

    // Create CSV content
    let content = "id,name,value\n1,Alice,100\n2,Bob,200\n";
    csv_file.write(Bytes::from(content)).await?;

    // 2. Create a bundle and attach the CSV file
    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(csv_file.url().as_str(), None).await?;

    // 3. Commit the bundle
    builder.commit("Initial commit").await?;

    // 4. Reload the bundle from disk (DO NOT modify the file)
    let loaded_bundle = Bundle::open(bundle_url.as_str(), None).await?;

    // 5. Query the bundle - should succeed
    let rows = loaded_bundle.num_rows().await?;
    assert_eq!(rows, 2, "Query should return 2 rows");

    // Verify we can also query the data
    let schema = loaded_bundle.schema().await?;
    assert_eq!(schema.fields().len(), 3, "Schema should have 3 fields");

    Ok(())
}

/// Test that version validation works with Parquet files too.
#[tokio::test]
async fn test_query_fails_when_source_parquet_changed() -> Result<(), BundlebaseError> {
    init();
    use arrow::array::{Int32Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use datafusion::parquet::arrow::ArrowWriter;
    use std::sync::Arc;

    // 1. Create a directory for our test data
    let data_dir = random_memory_dir();
    let parquet_file = data_dir.writable_file("test_data.parquet")?;

    // Helper function to create and write a parquet file
    async fn write_parquet(
        file: &dyn IOReadWriteFile,
        names: Vec<&str>,
        values: Vec<i32>,
    ) -> Result<(), BundlebaseError> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("value", DataType::Int32, false),
        ]));

        let name_array = StringArray::from(names);
        let value_array = Int32Array::from(values);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(name_array), Arc::new(value_array)],
        )
        .map_err(|e| BundlebaseError::from(e.to_string()))?;

        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, schema, None)
                .map_err(|e| BundlebaseError::from(e.to_string()))?;
            writer
                .write(&batch)
                .map_err(|e| BundlebaseError::from(e.to_string()))?;
            writer
                .close()
                .map_err(|e| BundlebaseError::from(e.to_string()))?;
        }

        file.write(Bytes::from(buffer)).await?;
        Ok(())
    }

    // Write initial parquet file
    write_parquet(&*parquet_file, vec!["Alice", "Bob"], vec![100, 200]).await?;

    // 2. Create a bundle and attach the parquet file
    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(parquet_file.url().as_str(), None).await?;

    // Verify initial query works
    let rows_before = builder.num_rows().await?;
    assert_eq!(rows_before, 2, "Initial query should return 2 rows");

    // 3. Commit the bundle
    builder.commit("Initial commit").await?;

    // 4. Modify the parquet file (changes the version)
    write_parquet(
        &*parquet_file,
        vec!["Alice", "Bob", "Carol"],
        vec![100, 200, 300],
    )
    .await?;

    // 5. Reload the bundle from disk
    let loaded_bundle = Bundle::open(bundle_url.as_str(), None).await?;

    // 6. Query the bundle - should fail with version mismatch
    let result = loaded_bundle.num_rows().await;

    // 7. Verify the error
    assert!(
        result.is_err(),
        "Query should fail when source parquet file has changed"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Version mismatch") || err_msg.contains("version mismatch"),
        "Error should mention version mismatch. Got: {}",
        err_msg
    );

    Ok(())
}

/// Test that multiple queries work correctly when file is unchanged.
#[tokio::test]
async fn test_multiple_queries_with_unchanged_source() -> Result<(), BundlebaseError> {
    init();
    // 1. Create a directory for our test data
    let data_dir = random_memory_dir();
    let csv_file = data_dir.writable_file("test_data.csv")?;

    // Create CSV content
    let content = "id,name,value\n1,Alice,100\n2,Bob,200\n3,Carol,300\n";
    csv_file.write(Bytes::from(content)).await?;

    // 2. Create a bundle and attach the CSV file
    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(csv_file.url().as_str(), None).await?;
    builder.commit("Initial commit").await?;

    // 3. Reload the bundle
    let loaded_bundle = Bundle::open(bundle_url.as_str(), None).await?;

    // 4. Run multiple queries - all should succeed
    // The version is validated once and cached
    let rows1 = loaded_bundle.num_rows().await?;
    assert_eq!(rows1, 3);

    let schema = loaded_bundle.schema().await?;
    assert_eq!(schema.fields().len(), 3);

    let rows2 = loaded_bundle.num_rows().await?;
    assert_eq!(rows2, 3);

    Ok(())
}

/// Regression test for the narrow-projection bypass (Phase 2.6) version
/// check. A `SELECT name FROM bundle` query takes the bypass path — it
/// reads only one column and skips the block cache. Without the explicit
/// `validate_version()` call added in the bypass, a modified source file
/// would silently return stale data.
#[tokio::test]
async fn test_narrow_projection_detects_version_change() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let csv_file = data_dir.writable_file("test_data.csv")?;

    let initial_content = "id,name,value\n1,Alice,100\n2,Bob,200\n";
    csv_file.write(Bytes::from(initial_content)).await?;

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(csv_file.url().as_str(), None).await?;
    builder.commit("Initial commit").await?;

    // Modify the source file AFTER commit — this bumps the effective version.
    let modified_content = "id,name,value\n1,Alice,100\n2,Bob,200\n3,Carol,300\n";
    csv_file.write(Bytes::from(modified_content)).await?;

    // Reload the bundle so the DataBlock is freshly built (no cached state).
    let loaded = Bundle::open(bundle_url.as_str(), None).await?;

    // Narrow projection — only one of three columns. This hits the Phase 2.6
    // bypass path in data_block.rs::scan().
    let result: Result<_, _> = async {
        let stream = loaded
            .query("SELECT name FROM bundle", vec![], None)
            .await?;
        let _batches: Vec<_> = stream.try_collect().await?;
        Ok::<_, BundlebaseError>(())
    }
    .await;

    assert!(
        result.is_err(),
        "narrow-projection query should fail on modified source"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Version mismatch") || err_msg.contains("version mismatch"),
        "error should be a version mismatch, got: {}",
        err_msg
    );

    Ok(())
}

/// Hot-block promotion: the first narrow query takes the bypass (no cache),
/// but the second narrow query on the same block falls through to Phase 3
/// and populates the block cache so further queries hit it. This test
/// verifies the overall behavior is correct — multiple narrow queries on
/// the same block all return the right data without error.
#[tokio::test]
async fn test_narrow_projection_hot_block_promotion() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let csv_file = data_dir.writable_file("test_data.csv")?;

    let content = "id,name,value\n1,Alice,100\n2,Bob,200\n3,Carol,300\n";
    csv_file.write(Bytes::from(content)).await?;

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(csv_file.url().as_str(), None).await?;
    builder.commit("Initial commit").await?;

    let loaded = Bundle::open(bundle_url.as_str(), None).await?;

    // Three narrow queries on the same block. First takes bypass, second
    // promotes to full cache population, third hits the cache.
    for i in 0..3 {
        let stream = loaded
            .query("SELECT name FROM bundle", vec![], None)
            .await?;
        let batches: Vec<_> = stream.try_collect().await?;
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 3, "query {} returned wrong row count", i);
    }

    Ok(())
}
