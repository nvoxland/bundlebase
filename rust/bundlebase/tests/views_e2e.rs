use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::{Bundle, BundleBuilder, BundlebaseError, BundleFacade, Operation};

#[tokio::test]
async fn test_attach_view_basic() -> Result<(), BundlebaseError> {
    // Create container and attach data
    let mut c = BundleBuilder::create(&random_memory_url().to_string()).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial data").await?;

    // Create view with select
    let adults = c.select("select * where age > 21", vec![]).await?;
    c.attach_view("adults", &adults).await?;
    c.commit("Add adults view").await?;

    // Open view
    let view = c.view("adults").await?;

    // Verify view has expected operations
    let operations = view.operations();
    println!("View has {} operations", operations.len());
    for (i, op) in operations.iter().enumerate() {
        println!("  Op {}: {}", i, op.describe());
    }

    // View should have: CREATE PACK, ATTACH, ATTACH VIEW (from parent), SELECT (from view)
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
    let c = BundleBuilder::create(&random_memory_url().to_string()).await?;

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
    let mut c = BundleBuilder::create(&container_url).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("v1").await?;

    let active_rs = c.select("select * where age > 21", vec![]).await?;
    c.attach_view("active", &active_rs).await?;
    c.commit("v2").await?;

    // Record initial view operations count
    let initial_view = c.view("active").await?;
    let initial_ops_count = initial_view.operations().len();
    println!("Initial operations count: {}", initial_ops_count);

    // Reopen container and add more data to parent
    let c_bundle = Bundle::open(&container_url).await?;
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
    let mut c = BundleBuilder::create(&random_memory_url().to_string()).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial data").await?;

    // Create view with multiple operations (select + filter)
    let mut filtered = c.select("select * where age > 21", vec![]).await?;
    filtered.filter("age < 65", vec![]).await?;

    c.attach_view("working_age", &filtered).await?;
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
    let mut c = BundleBuilder::create(&random_memory_url().to_string()).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("Initial").await?;

    // Create first view
    let adults1 = c.select("select * where age > 21", vec![]).await?;
    c.attach_view("adults", &adults1).await?;
    c.commit("Add first adults view").await?;

    // Try to create view with same name
    let adults2 = c.select("select * where age > 30", vec![]).await?;
    let result = c.attach_view("adults", &adults2).await;

    assert!(result.is_err());
    let err_msg = result.err().unwrap().to_string();
    assert!(err_msg.contains("View 'adults' already exists"));

    Ok(())
}

#[tokio::test]
async fn test_view_from_field_points_to_parent() -> Result<(), BundlebaseError> {
    // Create container and view
    let container_url = random_memory_url().to_string();
    let mut c = BundleBuilder::create(&container_url).await?;
    c.attach(&test_datafile("customers-0-100.csv"))
        .await?;
    c.commit("v1").await?;

    let active = c.select("select * where age > 21", vec![]).await?;
    c.attach_view("active", &active).await?;
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
