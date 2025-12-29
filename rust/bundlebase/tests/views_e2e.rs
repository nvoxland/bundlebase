use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::{Bundle, BundleBuilder, BundlebaseError, BundleFacade, Operation};

#[tokio::test]
async fn test_create_view_basic() -> Result<(), BundlebaseError> {
    // Create container and attach data
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial data").await?;

    // Create view with select
    let adults = c.select("select * where age > 21", vec![]).await?;
    c.create_view("adults", &adults).await?;
    c.commit("Add adults view").await?;

    // Open view
    let view = c.view("adults").await?;

    // Verify view has expected operations
    let operations = view.operations();
    println!("View has {} operations", operations.len());
    for (i, op) in operations.iter().enumerate() {
        println!("  Op {}: {}", i, op.describe());
    }

    // View should have: CREATE PACK, ATTACH, CREATE VIEW (from parent), SELECT (from view)
    assert!(operations.len() >= 4, "View should have at least 4 operations");

    // Verify SELECT operation is present
    let has_select = operations
        .iter()
        .any(|op| op.describe().to_lowercase().contains("select") && op.describe().contains("age > 21"));
    assert!(has_select, "View should have select operation");

    Ok(())
}

#[tokio::test]
async fn test_view_not_found() -> Result<(), BundlebaseError> {
    let c = BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Try to open non-existent view
    let result = c.view("nonexistent").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("View 'nonexistent' not found"));

    Ok(())
}

#[tokio::test]
async fn test_view_inherits_parent_changes() -> Result<(), BundlebaseError> {
    // Create container and view
    let container_url = random_memory_url().to_string();
    let mut c = BundleBuilder::create(&container_url, None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("v1").await?;

    let active_rs = c.select("select * where age > 21", vec![]).await?;
    c.create_view("active", &active_rs).await?;
    c.commit("v2").await?;

    // Record initial view operations count
    let initial_view = c.view("active").await?;
    let initial_ops_count = initial_view.operations().len();
    println!("Initial operations count: {}", initial_ops_count);

    // Reopen container and add more data to parent
    let c_bundle = Bundle::open(&container_url, None).await?;
    let mut c_reopened = c_bundle.extend(&container_url)?;
    c_reopened
        .attach(&test_datafile("customers-101-150.csv"))
        .await?;
    c_reopened.commit("v3 - more data").await?;

    // View should see new parent commits through FROM chain
    let view_after_parent_change: Bundle = c_reopened.view("active").await?;
    let new_ops_count = view_after_parent_change.operations().len();
    println!("Operations count after parent change: {}", new_ops_count);

    // The view should have more operations now (parent's new operations + view's select)
    assert!(
        new_ops_count > initial_ops_count,
        "View should inherit parent's new operations"
    );

    Ok(())
}

#[tokio::test]
async fn test_view_with_multiple_operations() -> Result<(), BundlebaseError> {
    // Create container
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial data").await?;

    // Create view with multiple operations (select + filter)
    let mut filtered = c.select("select * where age > 21", vec![]).await?;
    filtered.filter("age < 65", vec![]).await?;

    c.create_view("working_age", &filtered).await?;
    c.commit("Add working_age view").await?;

    // Open view and verify it has the operations
    let view = c.view("working_age").await?;
    let operations = view.operations();

    println!("View has {} operations:", operations.len());
    for (i, op) in operations.iter().enumerate() {
        println!("  Op {}: {}", i, op.describe());
    }

    // Should have at least the select and filter operations from the view
    // (plus any parent operations like attach)
    let select_ops = operations
        .iter()
        .filter(|op| op.describe().to_lowercase().contains("select"))
        .count();
    let filter_ops = operations
        .iter()
        .filter(|op| op.describe().contains("FILTER"))
        .count();

    assert_eq!(select_ops, 1, "View should have 1 select operation");
    assert_eq!(filter_ops, 1, "View should have 1 filter operation");

    Ok(())
}

#[tokio::test]
async fn test_duplicate_view_name() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial").await?;

    // Create first view
    let adults1 = c.select("select * where age > 21", vec![]).await?;
    c.create_view("adults", &adults1).await?;
    c.commit("Add first adults view").await?;

    // Try to create view with same name
    let adults2 = c.select("select * where age > 30", vec![]).await?;
    let result = c.create_view("adults", &adults2).await;

    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("View 'adults' already exists"));

    Ok(())
}

#[tokio::test]
async fn test_view_from_field_points_to_parent() -> Result<(), BundlebaseError> {
    // Create container and view
    let container_url = random_memory_url().to_string();
    let mut c = BundleBuilder::create(&container_url, None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("v1").await?;

    let active = c.select("select * where age > 21", vec![]).await?;
    c.create_view("active", &active).await?;
    c.commit("v2").await?;

    // Open the view
    let view = c.view("active").await?;

    // View's from() should point to the parent container
    let from_url = view.from();
    assert!(from_url.is_some(), "View should have a 'from' URL");

    // The from URL should match the parent container URL
    let from_str = from_url.unwrap().to_string();
    assert_eq!(
        from_str, container_url,
        "View's from URL should point to parent container"
    );

    Ok(())
}

#[tokio::test]
async fn test_view_has_parent_data() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial data").await?;

    let high_index = c.select("select * where \"Index\" > 50", vec![]).await?;
    c.create_view("high_index", &high_index).await?;
    c.commit("Add view").await?;

    let view = c.view("high_index").await?;

    // Debug assertions
    println!("View base_pack: {:?}", view.base_pack());
    println!("View data_packs count: {}", view.data_packs_count());
    println!("View operations: {:?}", view.operations().iter().map(|o| o.describe()).collect::<Vec<_>>());

    // Verify data is present
    assert!(view.base_pack().is_some(), "View should have base_pack from parent");
    assert!(view.data_packs_count() > 0, "View should have data_packs from parent");

    Ok(())
}

#[tokio::test]
async fn test_regular_container_select() -> Result<(), BundlebaseError> {
    // Test SELECT on a regular container (not a view) to isolate the issue
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv")).await?;
    c.commit("Initial data").await?;

    // Apply select operation
    c.select("select * where Country = 'Chile'", vec![]).await?;
    c.commit("After select").await?;

    // Try to get dataframe
    let df = c.dataframe().await?;
    let schema = df.schema();

    println!("Regular container schema: {:?}", schema);
    assert!(schema.fields().len() > 0, "Container should have schema after select");
    assert!(schema.field_with_name(None, "Country").is_ok(), "Container should have 'Country' column");

    Ok(())
}

#[tokio::test]
async fn test_view_dataframe_execution() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial data").await?;

    let chile = c.select("select * from data where Country = 'Chile'", vec![]).await?;
    c.create_view("chile", &chile).await?;
    c.commit("Add view").await?;

    let view = c.view("chile").await?;

    // This should work if data is inherited correctly
    let df = view.dataframe().await?;
    let schema = df.schema();

    assert!(schema.fields().len() > 0, "View dataframe should have schema");
    assert!(schema.field_with_name(None, "Country").is_ok(), "View should have 'Country' column");

    Ok(())
}

#[tokio::test]
async fn test_views_method() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv")).await?;
    c.commit("Initial data").await?;

    // Create multiple views
    let view1 = c.select("select * where \"Index\" > 50", vec![]).await?;
    c.create_view("high_index", &view1).await?;

    let view2 = c.select("select * where \"Index\" < 30", vec![]).await?;
    c.create_view("low_index", &view2).await?;

    c.commit("Add views").await?;

    // Get views map
    let views_map = c.views();

    assert_eq!(views_map.len(), 2, "Should have 2 views");

    // Check that both view names are present in the values
    let names: Vec<&String> = views_map.values().collect();
    assert!(names.contains(&&"high_index".to_string()));
    assert!(names.contains(&&"low_index".to_string()));

    Ok(())
}

#[tokio::test]
async fn test_view_lookup_by_name_and_id() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv")).await?;
    c.commit("Initial data").await?;

    // Create a view
    let adults = c.select("select * where age > 21", vec![]).await?;
    c.create_view("adults", &adults).await?;
    c.commit("Add adults view").await?;

    // Get the view ID
    let views_map = c.views();
    assert_eq!(views_map.len(), 1, "Should have 1 view");
    let (view_id, view_name) = views_map.iter().next().unwrap();
    assert_eq!(view_name, "adults");

    // Test 1: Open view by name
    let view_by_name = c.view("adults").await?;
    assert!(view_by_name.operations().len() >= 4, "View should have operations");

    // Test 2: Open view by ID
    let view_by_id = c.view(&view_id.to_string()).await?;
    assert!(view_by_id.operations().len() >= 4, "View should have operations");

    // Test 3: Both should return the same view (same number of operations)
    assert_eq!(
        view_by_name.operations().len(),
        view_by_id.operations().len(),
        "View opened by name and ID should have same operations"
    );

    // Test 4: Non-existent name should error with helpful message
    let result = c.view("nonexistent").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("View 'nonexistent' not found"), "Error should mention view not found");
    assert!(err_msg.contains("adults"), "Error should list available views");
    assert!(err_msg.contains(&view_id.to_string()), "Error should include view ID");

    // Test 5: Non-existent ID should error
    let result = c.view("00000000000000000000000000000000").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("View with ID"), "Error should mention ID not found");

    Ok(())
}
