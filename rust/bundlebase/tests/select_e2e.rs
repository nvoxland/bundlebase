use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use datafusion::scalar::ScalarValue;

mod common;

#[tokio::test]
async fn test_select_basic_filter() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Apply SQL query to filter results
    let queried = bundle
        .select(
            "SELECT first_name, last_name FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;

    // Try to query the filtered data
    let df = queried.dataframe().await?;
    let record_batches = df.as_ref().clone().collect().await?;
    assert!(
        !record_batches.is_empty(),
        "Should have at least one record batch"
    );

    Ok(())
}

#[tokio::test]
async fn test_select_is_not_required() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Apply SQL query to filter results
    let queried = bundle
        .select(
            "first_name, last_name FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;

    // Try to query the filtered data
    let df = queried.dataframe().await?;
    let record_batches = df.as_ref().clone().collect().await?;
    assert!(
        !record_batches.is_empty(),
        "Should have at least one record batch"
    );

    Ok(())
}

#[tokio::test]
async fn test_select_multiple_parameters() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Apply SQL query with multiple parameters
    let queried = bundle
        .select(
            "SELECT id, first_name FROM bundle WHERE salary > $1 OR gender = $2",
            vec![
                ScalarValue::Float64(Some(100000.0)),
                ScalarValue::Utf8(Some("F".to_string())),
            ],
        )
        .await?;

    let df = queried.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1);
    assert!(
        result[0].num_rows() > 0,
        "Should have results matching either condition"
    );

    Ok(())
}
#[tokio::test]
async fn test_select_no_parameters() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Apply SQL query without parameters
    let queried = bundle.select("SELECT * FROM bundle LIMIT 10", vec![]).await?;

    let df = queried.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].num_rows(), 10);

    Ok(())
}
#[tokio::test]
async fn test_select_with_aggregation() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Apply SQL query with GROUP BY
    let queried = bundle
        .select(
            "SELECT gender, COUNT(*) as count FROM bundle GROUP BY gender",
            vec![],
        )
        .await?;

    let df = queried.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert!(!result.is_empty(), "Should have at least one batch");
    let total_rows: usize = result.iter().map(|batch| batch.num_rows()).sum();
    assert!(total_rows > 0, "Should have aggregation results");

    Ok(())
}

#[tokio::test]
async fn test_explain_basic() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Explain should return a non-empty string
    let plan = bundle.bundle.explain().await?;
    assert!(
        !plan.is_empty(),
        "Explain should return a non-empty query plan"
    );
    // Check for the formatted plan with type markers
    assert!(
        plan.contains("***"),
        "Explain should contain plan type markers (*** ***)"
    );

    Ok(())
}

#[tokio::test]
async fn test_explain_with_filter() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Apply a filter and explain
    let filtered = bundle
        .filter("salary > $1", vec![ScalarValue::Float64(Some(50000.0))])
        .await?;
    let plan = filtered.bundle.explain().await?;

    assert!(
        !plan.is_empty(),
        "Explain should return plan for filtered bundle"
    );
    assert!(plan.len() > 0, "Explain should produce meaningful output");

    Ok(())
}
