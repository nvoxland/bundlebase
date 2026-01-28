use bundlebase::io::read_yaml;
use bundlebase::test_utils::{
    assert_vec_regexp, describe_ops, field_names, random_memory_url, test_datafile,
};
use bundlebase::{Bundle, BundleBuilder, BundleFacade, BundlebaseError, Operation};

#[tokio::test]
async fn test_create_view_basic() -> Result<(), BundlebaseError> {
    // Create container and attach data
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create view with SQL
    c.create_view("chile", "select * from bundle where Country = 'Chile'").await?;
    c.commit("Add chile view").await?;

    // Open view
    let view = c.view("chile").await?;

    assert_vec_regexp(
        vec![
            "ATTACH: memory:///test_data/customers-0-100.csv",
            "CREATE VIEW: 'chile'",
            "select \\* from bundle where Country = 'Chile'",
        ],
        describe_ops(view.as_ref()),
    );

    let schema = view.schema().await?;
    assert_eq!(
        vec![
            "Index",
            "Customer Id",
            "First Name",
            "Last Name",
            "Company",
            "City",
            "Country",
            "Phone 1",
            "Phone 2",
            "Email",
            "Subscription Date",
            "Website"
        ],
        field_names(&schema)
    );

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
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("v1").await?;

    c.create_view("active", "select * from bundle where \"Index\" > 21").await?;
    c.commit("v2").await?;

    // Record initial view operations count
    let initial_view = c.view("active").await?;
    let initial_ops_count = initial_view.operations().len();
    println!("Initial operations count: {}", initial_ops_count);

    // Reopen container and add more data to parent
    let c_bundle = Bundle::open(&container_url, None).await?;
    let mut c_reopened = c_bundle.extend(None)?;
    c_reopened
        .attach(&test_datafile("customers-101-150.csv"), None)
        .await?;
    c_reopened.commit("v3 - more data").await?;

    // View should see new parent commits through FROM chain
    let view_after_parent_change = c_reopened.view("active").await?;
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
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create view with SQL that has multiple conditions
    c.create_view("working_age", "select * from bundle where \"Index\" > 21 AND \"Index\" < 65").await?;
    c.commit("Add working_age view").await?;

    // Open view and verify it has the operations
    let view = c.view("working_age").await?;
    let operations = view.operations();

    println!("View has {} operations:", operations.len());
    for (i, op) in operations.iter().enumerate() {
        println!("  Op {}: {}", i, op.describe());
    }

    // Should have the select operation from the view
    // (plus any parent operations like attach)
    let select_ops = operations
        .iter()
        .filter(|op| op.describe().to_lowercase().contains("select"))
        .count();

    assert_eq!(select_ops, 1, "View should have 1 select operation");

    Ok(())
}

#[tokio::test]
async fn test_duplicate_view_name() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial").await?;

    // Create first view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add first adults view").await?;

    // Try to create view with same name
    let result = c.create_view("adults", "select * from bundle where \"Index\" > 30").await;

    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("View 'adults' already exists"));

    Ok(())
}

#[tokio::test]
async fn test_view_has_view_field_in_init() -> Result<(), BundlebaseError> {
    use bundlebase::bundle::{InitCommit, INIT_FILENAME, META_DIR};

    // Create container and view
    let container_url = random_memory_url().to_string();
    let mut c = BundleBuilder::create(&container_url, None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("v1").await?;

    c.create_view("active", "select * from bundle where \"Index\" > 21").await?;
    c.commit("v2").await?;

    // Get the view ID
    let views_map = c.views();
    let (view_id, _) = views_map.iter().next().unwrap();

    // Read the view's init file
    let view_dir = c
        .data_dir()
        .subdir(&format!("view_{}", view_id))?;
    let init_file = view_dir.subdir(META_DIR)?.file(INIT_FILENAME)?;
    let init_commit: Option<InitCommit> = read_yaml(init_file.as_ref()).await?;
    let init_commit = init_commit.expect("View should have init file");

    // View should have view field set, not from field
    assert!(init_commit.view.is_some(), "View should have 'view' field");
    assert_eq!(
        init_commit.view.unwrap(),
        view_id.to_string(),
        "View field should match view ID"
    );
    assert!(
        init_commit.from.is_none(),
        "View should not have 'from' field"
    );
    assert!(init_commit.id.is_none(), "View should not have 'id' field");

    Ok(())
}

#[tokio::test]
async fn test_view_has_parent_data() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    c.create_view("high_index", "select * where \"Index\" > 50").await?;
    c.commit("Add view").await?;

    let view = c.view("high_index").await?;

    // Debug assertions
    println!("View data_packs count: {}", view.packs_count());
    println!(
        "View operations: {:?}",
        view.operations()
            .iter()
            .map(|o| o.describe())
            .collect::<Vec<_>>()
    );

    assert!(
        view.packs_count() > 0,
        "View should have data_packs from parent"
    );

    Ok(())
}

#[tokio::test]
async fn test_view_is_marked_as_view() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Container should not be marked as a view
    assert!(!c.bundle().is_view(), "Container should not be marked as view");

    // Create a view
    c.create_view("filtered", "select * from bundle limit 10").await?;
    c.commit("Add view").await?;

    // Open the view
    let view = c.view("filtered").await?;

    // View should be marked as a view
    assert!(view.is_view(), "View should be marked as view");

    // Verify view has access to data (even though it's marked as a view)
    let df = view.dataframe().await?;
    let count = (*df).clone().count().await?;
    assert!(count > 0, "View should have access to data");

    Ok(())
}

#[tokio::test]
async fn test_cannot_attach_to_view() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view
    c.create_view("filtered", "select * from bundle limit 10").await?;
    c.commit("Add view").await?;

    // Open the view
    let view_bundle = Bundle::open(
        &c.data_dir().as_ref().subdir(&format!("view_{}", c.views().keys().next().unwrap()))?.url().to_string(),
        None,
    ).await?;
    let mut view_builder = view_bundle.extend(Some(random_memory_url().as_str()))?;

    // Try to attach data to the view - should fail
    let result = view_builder.attach(&test_datafile("customers-101-150.csv"), None).await;
    assert!(result.is_err(), "Should not be able to attach to a view");

    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("is not allowed on a view"),
        "Error message should mention operation not allowed on view, got: {}",
        err_msg
    );

    Ok(())
}

#[tokio::test]
async fn test_cannot_create_view_on_view() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view
    c.create_view("filtered", "select * from bundle limit 10").await?;
    c.commit("Add view").await?;

    // Open the view
    let view_bundle = Bundle::open(
        &c.data_dir().as_ref().subdir(&format!("view_{}", c.views().keys().next().unwrap()))?.url().to_string(),
        None,
    ).await?;
    let mut view_builder = view_bundle.extend(Some(random_memory_url().as_str()))?;

    // Try to create a view on the view - should fail
    let result = view_builder.create_view("subview", "select * limit 5").await;
    assert!(result.is_err(), "Should not be able to create view on a view");

    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("is not allowed on a view"),
        "Error message should mention operation not allowed on view, got: {}",
        err_msg
    );

    Ok(())
}

#[tokio::test]
async fn test_cannot_drop_view_from_view() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create two views
    c.create_view("view1", "select * from bundle limit 10").await?;
    c.create_view("view2", "select * from bundle limit 20").await?;
    c.commit("Add views").await?;

    // Open view1
    // views() returns HashMap<ObjectId, String> where key is ID and value is name
    let views = c.views();
    let view1_id = views.iter().find(|(_, name)| name.as_str() == "view1").map(|(id, _)| id).unwrap();
    let view_bundle = Bundle::open(
        &c.data_dir().as_ref().subdir(&format!("view_{}", view1_id))?.url().to_string(),
        None,
    ).await?;
    let mut view_builder = view_bundle.extend(Some(random_memory_url().as_str()))?;

    // Try to drop view2 from view1 - should fail
    let result = view_builder.drop_view("view2").await;
    assert!(result.is_err(), "Should not be able to drop view from a view");

    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("is not allowed on a view"),
        "Error message should mention operation not allowed on view, got: {}",
        err_msg
    );

    Ok(())
}

#[tokio::test]
async fn test_regular_container_select() -> Result<(), BundlebaseError> {
    // Test SELECT on a regular container (not a view) to isolate the issue
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Apply filter operation
    c.filter("SELECT * FROM bundle WHERE Country = 'Chile'", vec![]).await?;
    c.commit("After filter").await?;

    // Try to get dataframe
    let df = c.dataframe().await?;
    let schema = df.schema();

    println!("Regular container schema: {:?}", schema);
    assert!(
        schema.fields().len() > 0,
        "Container should have schema after select"
    );
    assert!(
        schema.field_with_name(None, "Country").is_ok(),
        "Container should have 'Country' column"
    );

    Ok(())
}

#[tokio::test]
async fn test_view_dataframe_execution() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    c.create_view("chile", "select * from bundle where Country = 'Chile'").await?;
    c.commit("Add view").await?;

    let view = c.view("chile").await?;

    // This should work if data is inherited correctly
    let df = view.dataframe().await?;
    let schema = df.schema();

    assert!(
        schema.fields().len() > 0,
        "View dataframe should have schema"
    );
    assert!(
        schema.field_with_name(None, "Country").is_ok(),
        "View should have 'Country' column"
    );

    Ok(())
}

#[tokio::test]
async fn test_views_method() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create multiple views
    c.create_view("high_index", "select * where \"Index\" > 50").await?;
    c.create_view("low_index", "select * where \"Index\" < 30").await?;

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
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add adults view").await?;

    // Get the view ID
    let views_map = c.views();
    assert_eq!(views_map.len(), 1, "Should have 1 view");
    let (view_id, view_name) = views_map.iter().next().unwrap();
    assert_eq!(view_name, "adults");

    // Test 1: Open view by name
    let view_by_name = c.view("adults").await?;
    assert!(
        view_by_name.operations().len() >= 3,
        "View should have operations"
    );

    // Test 2: Open view by ID
    let view_by_id = c.view(&view_id.to_string()).await?;
    assert!(
        view_by_id.operations().len() >= 3,
        "View should have operations"
    );

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
    assert!(
        err_msg.contains("View 'nonexistent' not found"),
        "Error should mention view not found"
    );
    assert!(
        err_msg.contains("adults"),
        "Error should list available views"
    );
    assert!(
        err_msg.contains(&view_id.to_string()),
        "Error should include view ID"
    );

    // Test 5: Non-existent ID should error
    let result = c.view("00000000000000000000000000000000").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("View with ID"),
        "Error should mention ID not found"
    );

    Ok(())
}

#[tokio::test]
async fn test_rename_view_basic() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add adults view").await?;

    // Rename the view
    c.rename_view("adults", "adults_view").await?;
    c.commit("Renamed view").await?;

    // Verify old name doesn't work
    let result = c.view("adults").await;
    assert!(result.is_err());
    assert!(result.err().unwrap().to_string().contains("not found"));

    // Verify new name works
    let view = c.view("adults_view").await?;
    assert!(view.operations().len() >= 4);

    // Verify views() returns new name
    let views_map = c.views();
    assert_eq!(views_map.len(), 1);
    let view_name = views_map.values().next().unwrap();
    assert_eq!(view_name, "adults_view");

    Ok(())
}

#[tokio::test]
async fn test_rename_view_old_name_not_found() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Try to rename non-existent view
    let result = c.rename_view("nonexistent", "new_name").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("View 'nonexistent' not found"),
        "Error should mention view not found"
    );

    Ok(())
}

#[tokio::test]
async fn test_rename_view_new_name_exists() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create two views
    c.create_view("view1", "select * from bundle where \"Index\" > 21").await?;
    c.create_view("view2", "select * from bundle where \"Index\" < 30").await?;
    c.commit("Add two views").await?;

    // Try to rename view1 to view2 (conflict)
    let result = c.rename_view("view1", "view2").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("already exists"),
        "Error should mention view already exists"
    );

    Ok(())
}

#[tokio::test]
async fn test_rename_view_preserves_view_data() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view and get its dataframe
    c.create_view("high_index", "select * from bundle where \"Index\" > 50").await?;
    c.commit("Add view").await?;

    let view_before = c.view("high_index").await?;
    let df_before = view_before.dataframe().await?;
    let rows_before = (*df_before).clone().count().await?;

    // Rename the view
    c.rename_view("high_index", "high_values").await?;
    c.commit("Renamed view").await?;

    // Verify data is still accessible under new name
    let view_after = c.view("high_values").await?;
    let df_after = view_after.dataframe().await?;
    let rows_after = (*df_after).clone().count().await?;

    assert_eq!(
        rows_before, rows_after,
        "View should have same row count after rename"
    );

    Ok(())
}

#[tokio::test]
async fn test_rename_view_commit_and_reopen() -> Result<(), BundlebaseError> {
    let container_url = random_memory_url().to_string();
    let mut c = BundleBuilder::create(&container_url, None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create and rename a view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add adults view").await?;

    c.rename_view("adults", "adults_renamed").await?;
    c.commit("Renamed view").await?;

    // Reopen the bundle
    let bundle = Bundle::open(&container_url, None).await?;

    // Verify old name doesn't exist
    let result = bundle.view("adults").await;
    assert!(result.is_err());

    // Verify new name works
    let view = bundle.view("adults_renamed").await?;
    assert!(view.operations().len() >= 4);

    // Verify views() shows correct name
    let views_map = bundle.views();
    assert_eq!(views_map.len(), 1);
    let view_name = views_map.values().next().unwrap();
    assert_eq!(view_name, "adults_renamed");

    Ok(())
}

#[tokio::test]
async fn test_create_view_with_sql() -> Result<(), BundlebaseError> {
    // This test verifies that create_view properly stores the SQL as a select operation

    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial").await?;

    // Create view with SQL directly
    c.create_view("limited", "select * from bundle limit 10").await?;
    c.commit("Added view").await?;

    // Verify the view has the select operation
    let bundle = Bundle::open(c.url().as_str(), None).await?;
    let view = bundle.view("limited").await?;
    let view_ops = view.operations();
    let has_select = view_ops.iter().any(|op| op.describe().to_lowercase().contains("select"));
    assert!(has_select, "View should contain the select operation");

    Ok(())
}

#[tokio::test]
async fn test_drop_view_basic() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add adults view").await?;

    // Verify view exists
    assert!(c.view("adults").await.is_ok());
    let views_map = c.views();
    assert_eq!(views_map.len(), 1);

    // Drop the view
    c.drop_view("adults").await?;
    c.commit("Dropped view").await?;

    // Verify view no longer exists
    let result = c.view("adults").await;
    assert!(result.is_err());
    assert!(result.err().unwrap().to_string().contains("not found"));

    // Verify views map is empty
    let views_map = c.views();
    assert_eq!(views_map.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_drop_view_not_found() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Try to drop non-existent view
    let result = c.drop_view("nonexistent").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("View 'nonexistent' not found"),
        "Error should mention view not found"
    );

    Ok(())
}

#[tokio::test]
async fn test_drop_view_commit_and_reopen() -> Result<(), BundlebaseError> {
    let container_url = random_memory_url().to_string();
    let mut c = BundleBuilder::create(&container_url, None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create and drop a view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add adults view").await?;

    c.drop_view("adults").await?;
    c.commit("Dropped view").await?;

    // Reopen the bundle
    let bundle = Bundle::open(&container_url, None).await?;

    // Verify view doesn't exist
    let result = bundle.view("adults").await;
    assert!(result.is_err());

    // Verify views map is empty
    let views_map = bundle.views();
    assert_eq!(views_map.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_drop_view_preserves_other_views() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create two views
    c.create_view("view1", "select * from bundle where \"Index\" > 21").await?;
    c.create_view("view2", "select * from bundle where \"Index\" < 30").await?;
    c.commit("Add two views").await?;

    // Verify both views exist
    assert_eq!(c.views().len(), 2);

    // Drop one view
    c.drop_view("view1").await?;
    c.commit("Dropped view1").await?;

    // Verify view1 is gone but view2 remains
    let result = c.view("view1").await;
    assert!(result.is_err());

    let view2_after = c.view("view2").await?;
    assert!(view2_after.operations().len() >= 4);

    // Verify views map only contains view2
    let views_map = c.views();
    assert_eq!(views_map.len(), 1);
    let remaining_view = views_map.values().next().unwrap();
    assert_eq!(remaining_view, "view2");

    Ok(())
}

#[tokio::test]
async fn test_drop_view_twice_fails() -> Result<(), BundlebaseError> {
    let mut c = BundleBuilder::create(random_memory_url().as_str(), None).await?;
    c.attach(&test_datafile("customers-0-100.csv"), None).await?;
    c.commit("Initial data").await?;

    // Create a view
    c.create_view("adults", "select * from bundle where \"Index\" > 21").await?;
    c.commit("Add adults view").await?;

    // Drop the view
    c.drop_view("adults").await?;
    c.commit("Dropped view").await?;

    // Try to drop it again
    let result = c.drop_view("adults").await;
    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("View 'adults' not found"),
        "Error should mention view not found"
    );

    Ok(())
}
