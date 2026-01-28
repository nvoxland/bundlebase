use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use datafusion::scalar::ScalarValue;
use futures::TryStreamExt;

mod common;

#[tokio::test]
async fn test_filter_basic() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Filter: salary > 50000
    bundle
        .filter(
            "SELECT * FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;

    assert_eq!(798, bundle.num_rows().await?);

    // Add a second filter (salary > 50000 AND salary < 150000)
    bundle
        .filter(
            "SELECT * FROM bundle WHERE salary < $1",
            vec![ScalarValue::Float64(Some(150000.0))],
        )
        .await?;

    assert_eq!(338, bundle.num_rows().await?);

    Ok(())
}
#[tokio::test]
async fn test_filter_multiple_parameters() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Filter: salary > 50000 AND first_name = 'John'
    let filtered = bundle
        .filter(
            "SELECT * FROM bundle WHERE salary > $1 AND first_name = $2",
            vec![
                ScalarValue::Float64(Some(50000.0)),
                ScalarValue::Utf8(Some("John".to_string())),
            ],
        )
        .await?;

    // Try to query
    let df = filtered.dataframe().await?;
    let _result = df.as_ref().clone().collect().await?;

    Ok(())
}
#[tokio::test]
async fn test_filter_preserves_schema() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Store schema before filter (bundle will be moved)
    let num_fields_before = bundle.schema().await?.fields().len();

    // Apply filter
    let filtered = bundle
        .filter(
            "SELECT * FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;

    // Schema should be the same (filter doesn't change schema, only reduces rows)
    let df = filtered.dataframe().await?;
    let schema_after = df.schema();

    // Verify we still have the same columns
    assert_eq!(
        num_fields_before,
        schema_after.fields().len(),
        "Schema should have same number of fields"
    );

    Ok(())
}
#[tokio::test]
async fn test_filter_with_other_operations() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Apply filter then remove a column
    let filtered = bundle
        .filter(
            "SELECT * FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;

    let reduced = filtered.drop_column("email").await?;

    // Query should work
    let df = reduced.dataframe().await?;
    let _result = df.as_ref().clone().collect().await?;

    Ok(())
}

#[tokio::test]
async fn test_select_limit() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Query with LIMIT
    let stream = bundle
        .query("SELECT * FROM bundle LIMIT 10", vec![])
        .await?;
    let record_batches: Vec<_> = stream.try_collect().await?;
    let total_rows: usize = record_batches.iter().map(|rb| rb.num_rows()).sum();

    assert_eq!(
        total_rows, 10,
        "Query with LIMIT 10 should return exactly 10 rows"
    );

    Ok(())
}

#[tokio::test]
async fn test_select_with_filter() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Query with WHERE clause
    let stream = bundle
        .query(
            "SELECT id, salary FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;
    let record_batches: Vec<_> = stream.try_collect().await?;

    assert!(!record_batches.is_empty(), "Should have results");
    assert_eq!(record_batches[0].num_columns(), 2, "Should have 2 columns");

    Ok(())
}
