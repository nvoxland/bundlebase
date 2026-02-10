use arrow::array::{Int32Array, StringArray};
use bundlebase::bundle::BundleFacade;
use bundlebase::IndexType;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::{Bundle, BundleBuilder};
use futures::StreamExt;
use futures::TryStreamExt;

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

    // Query via query() - should return record batches with schema
    let stream = bundle.query("SELECT * FROM bundle", vec![]).await.unwrap();
    let batches: Vec<_> = stream.try_collect().await.unwrap();

    // Verify it works
    assert!(!batches.is_empty(), "Should have at least one batch");
    let schema = batches[0].schema();
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

    // Query via query()
    let stream = bundle.query("SELECT * FROM bundle", vec![]).await.unwrap();
    let batches: Vec<_> = stream.try_collect().await.unwrap();
    assert!(!batches.is_empty(), "Should have at least one batch");
    let result_schema = batches[0].schema();

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

#[tokio::test]
async fn test_bundle_status_table_empty() {
    let data_dir = random_memory_dir();
    let bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Query the bundle_info.status table - should be empty since no changes yet
    let df = bundle.bundle().ctx().sql("SELECT * FROM bundle_info.status").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 4, "bundle_info.status should have 4 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "change_id", "description", "operation_count"]);

    // Verify no rows (no uncommitted changes)
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 0, "No uncommitted changes should exist");
}

#[tokio::test]
async fn test_bundle_status_table_with_uncommitted_changes() {
    use bundlebase::BundleFacade;

    let data_dir = random_memory_dir();
    let bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Make some changes but don't commit
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Query the bundle_info.status table via SQL - now returns actual uncommitted changes
    let df = bundle.bundle().ctx().sql("SELECT * FROM bundle_info.status").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    // SQL table now returns uncommitted changes (previously was always empty)
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "SQL bundle_info.status now returns uncommitted changes");

    // Use the status() method to check uncommitted changes - should match SQL
    let status = bundle.status();
    let changes = status.changes();
    assert_eq!(changes.len(), 1, "One uncommitted change should exist via status()");
    assert!(changes[0].description.contains("ATTACH"), "Description should mention ATTACH");
    assert!(changes[0].operations.len() >= 1, "Should have at least 1 operation");
}

#[tokio::test]
async fn test_bundle_status_table_multiple_changes() {
    use bundlebase::BundleFacade;

    let data_dir = random_memory_dir();
    let bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Make multiple changes but don't commit
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.set_name("Test Bundle").await.unwrap();
    bundle.set_description("A test bundle").await.unwrap();

    // Query the bundle_info.status table via SQL - now returns actual uncommitted changes
    let df = bundle.bundle().ctx().sql("SELECT id, description, operation_count FROM bundle_info.status ORDER BY id").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    // SQL table now returns uncommitted changes (previously was always empty)
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 3, "SQL bundle_info.status now returns uncommitted changes");

    // Use the status() method to check uncommitted changes - should match SQL
    let status = bundle.status();
    let changes = status.changes();
    assert_eq!(changes.len(), 3, "Three uncommitted changes should exist via status()");
}

#[tokio::test]
async fn test_bundle_status_table_cleared_after_commit() {
    use bundlebase::BundleFacade;

    let data_dir = random_memory_dir();
    let bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Make changes
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Verify uncommitted changes exist before commit via status()
    let status_before = bundle.status();
    assert_eq!(status_before.changes().len(), 1, "Should have 1 uncommitted change before commit");

    // Commit
    bundle.commit("Initial commit").await.unwrap();

    // Verify no uncommitted changes after commit via status()
    let status_after = bundle.status();
    assert_eq!(status_after.changes().len(), 0, "Should have no uncommitted changes after commit");
}

#[tokio::test]
async fn test_bundle_status_table_readonly_bundle() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Commit some changes
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    // Open as read-only Bundle
    let readonly_bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.status table - should be empty since Bundle doesn't track uncommitted changes
    let df = readonly_bundle.ctx().sql("SELECT * FROM bundle_info.status").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 0, "Read-only bundle should have no uncommitted changes");
}

// ==================== bundle_info.details tests ====================

#[tokio::test]
async fn test_bundle_details_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.details table
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.details").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 6, "bundle_info.details should have 6 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "name", "description", "url", "from", "version"]);
}

#[tokio::test]
async fn test_bundle_details_table_single_row() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.set_name("Test Bundle").await.unwrap();
    bundle.set_description("A test bundle description").await.unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.details table
    let df = bundle.ctx().sql("SELECT id, name, description, version FROM bundle_info.details").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    // Verify exactly one row
    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "bundle_info.details should have exactly 1 row");

    // Verify values
    let batch = batches[0].as_ref().unwrap();
    let name_col = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
    let desc_col = batch.column(2).as_any().downcast_ref::<StringArray>().unwrap();

    assert_eq!(name_col.value(0), "Test Bundle");
    assert_eq!(desc_col.value(0), "A test bundle description");
}

// ==================== bundle_info.views tests ====================

#[tokio::test]
async fn test_bundle_views_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.views table
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.views").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 2, "bundle_info.views should have 2 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "name"]);
}

#[tokio::test]
async fn test_bundle_views_table_empty() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.views table - should be empty
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.views").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 0, "bundle_info.views should be empty when no views exist");
}

#[tokio::test]
async fn test_bundle_views_table_with_views() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Create view with SQL
    bundle.create_view("high_earners", "SELECT * FROM bundle WHERE salary >= 100000").await.unwrap();
    bundle.commit("Initial commit with view").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.views table
    let df = bundle.ctx().sql("SELECT name FROM bundle_info.views").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "bundle_info.views should have 1 view");

    let batch = batches[0].as_ref().unwrap();
    let name_col = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(name_col.value(0), "high_earners");
}

// ==================== bundle_info.indexes tests ====================

#[tokio::test]
async fn test_bundle_indexes_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.indexes table
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.indexes").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 4, "bundle_info.indexes should have 4 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "column", "type", "tokenizer"]);
}

#[tokio::test]
async fn test_bundle_indexes_table_empty() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.indexes table - should be empty
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.indexes").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 0, "bundle_info.indexes should be empty when no indexes exist");
}

#[tokio::test]
async fn test_bundle_indexes_table_with_index() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.create_index("id", IndexType::Column).await.unwrap();
    bundle.commit("Initial commit with index").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.indexes table
    let df = bundle.ctx().sql("SELECT column, type FROM bundle_info.indexes").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "bundle_info.indexes should have 1 index");

    let batch = batches[0].as_ref().unwrap();
    let column_col = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
    let type_col = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(column_col.value(0), "id");
    assert_eq!(type_col.value(0), "column");
}

// ==================== bundle_info.packs tests ====================

#[tokio::test]
async fn test_bundle_packs_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.packs table
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.packs").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 4, "bundle_info.packs should have 4 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "name", "join_type", "expression"]);
}

#[tokio::test]
async fn test_bundle_packs_table_base_pack() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.packs table - should have base pack
    let df = bundle.ctx().sql("SELECT name, join_type FROM bundle_info.packs").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "bundle_info.packs should have base pack");

    let batch = batches[0].as_ref().unwrap();
    let name_col = batch.column(0).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(name_col.value(0), "base");
}

// ==================== bundle_info.blocks tests ====================

#[tokio::test]
async fn test_bundle_blocks_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.blocks table
    let df = bundle.ctx().sql("SELECT * FROM bundle_info.blocks").await.unwrap();

    // Verify schema has the expected columns
    let schema = df.schema();
    assert_eq!(schema.fields().len(), 7, "bundle_info.blocks should have 7 columns");

    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(field_names, vec!["id", "version", "pack_id", "pack_name", "source_id", "source_location", "source_version"]);
}

#[tokio::test]
async fn test_bundle_blocks_table_with_data() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    let bundle = Bundle::open(data_dir.url().as_str(), None).await.unwrap();

    // Query the bundle_info.blocks table
    let df = bundle.ctx().sql("SELECT id, pack_name FROM bundle_info.blocks").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 1, "bundle_info.blocks should have 1 block");

    let batch = batches[0].as_ref().unwrap();
    let pack_name_col = batch.column(1).as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(pack_name_col.value(0), "base");
}

#[tokio::test]
async fn test_bundle_blocks_table_empty() {
    let data_dir = random_memory_dir();
    let bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Don't attach any data - blocks should be empty
    // Query the bundle_info.blocks table
    let df = bundle.bundle().ctx().sql("SELECT * FROM bundle_info.blocks").await.unwrap();
    let batches: Vec<_> = df.execute_stream().await.unwrap().collect::<Vec<_>>().await;

    let total_rows: usize = batches.iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|b| b.num_rows())
        .sum();
    assert_eq!(total_rows, 0, "bundle_info.blocks should be empty when no data attached");
}
