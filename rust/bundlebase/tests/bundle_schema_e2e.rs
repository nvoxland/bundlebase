use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::{Bundle, BundleBuilder};
use futures::StreamExt;

#[tokio::test]
async fn test_bundle_data_table() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Populate cache by attaching data and getting the dataframe
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    let df = bundle.dataframe().await.unwrap();

    // Debug: Check if cache is populated
    let df_fields = df.schema().fields().len();
    println!("DataFrame schema has {} fields", df_fields);
    assert!(df_fields > 0, "DataFrame should have fields");

    // Query via select - should return the cached dataframe
    let result = bundle.select("SELECT * FROM bundle", vec![]).await.unwrap();
    let result_df = result.dataframe().await.unwrap();

    // Verify it works
    let schema = result_df.schema();
    println!("Result schema has {} fields", schema.fields().len());
    assert!(schema.fields().len() > 0, "Schema should have fields");
}

#[tokio::test]
async fn test_data_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Attach data
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Get dataframe to populate cache
    let df = bundle.dataframe().await.unwrap();
    let df_schema = df.schema();

    // Query via select
    let result = bundle.select("SELECT * FROM bundle", vec![]).await.unwrap();
    let result_df = result.dataframe().await.unwrap();
    let result_schema = result_df.schema();

    // Schemas should match
    assert_eq!(
        df_schema.fields().len(),
        result_schema.fields().len(),
        "Data table schema should match dataframe schema"
    );

    // Check field names match
    for (df_field, result_field) in df_schema.fields().iter().zip(result_schema.fields().iter()) {
        assert_eq!(
            df_field.name(),
            result_field.name(),
            "Field names should match"
        );
    }
}

#[tokio::test]
async fn test_bundle_history_table_empty() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Attach data and commit so we can open the bundle
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    // Re-open the bundle
    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.history table directly via ctx
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.history").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 6, "bundle_info.history should have 6 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "url", "author", "message", "timestamp", "change_count"]);

    // Verify one commit exists (the initial commit)
    let batches: Vec<_> = df.clone().execute_stream().await.unwrap().collect::<Vec<_>>().await;
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "One commit should exist");
}

#[tokio::test]
async fn test_bundle_history_table_with_commit() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Attach data and commit
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("First commit").await.unwrap();

    // Re-open the bundle to see the commit
    let bundle = Bundle::open(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Query the bundle_info.history table directly via ctx
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.history").await.unwrap();

    // Verify one commit exists
    let batches: Vec<_> = df.clone().execute_stream().await.unwrap().collect::<Vec<_>>().await;
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "One commit should exist");

    // Query specific columns
    let df = bundle.ctx().sql("SELECT message, change_count FROM bundle_info.history").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    // Verify message column value
    let batch = batches[0].as_ref().unwrap();
    let message_col = batch.column(0).as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
    assert_eq!(message_col.value(0), "First commit");
}

#[tokio::test]
async fn test_bundle_history_table_multiple_commits() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // First commit
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial data load").await.unwrap();

    // Second commit
    bundle.set_name("Test Bundle").await.unwrap();
    bundle.commit("Set bundle name").await.unwrap();

    // Re-open the bundle
    let bundle = Bundle::open(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Query the bundle_info.history table directly via ctx
    let df = bundle.ctx().sql("SELECT id, message FROM bundle_info.history ORDER BY id").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    // Verify two commits exist
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 2, "Two commits should exist");

    // Verify messages
    let batch = batches[0].as_ref().unwrap();
    let message_col = batch.column(1).as_any().downcast_ref::<arrow::array::StringArray>().unwrap();
    assert_eq!(message_col.value(0), "Initial data load");
    assert_eq!(message_col.value(1), "Set bundle name");
}
