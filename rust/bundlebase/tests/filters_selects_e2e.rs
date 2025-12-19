use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{field_names, random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use datafusion::scalar::ScalarValue;

mod common;

#[tokio::test]
async fn test_filter_basic() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Filter: salary > 50000
    let filtered = bundle
        .filter("salary > $1", vec![ScalarValue::Float64(Some(50000.0))])
        .await?;

    // Try to query the filtered data
    let df = filtered.dataframe().await?;
    let record_batches = df.as_ref().clone().collect().await?;
    assert!(
        !record_batches.is_empty(),
        "Should have at least one record batch"
    );

    Ok(())
}
#[tokio::test]
async fn test_filter_multiple_parameters() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Filter: salary > 50000 AND first_name = 'John'
    let filtered = bundle
        .filter(
            "salary > $1 AND first_name = $2",
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
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Store schema before filter (bundle will be moved)
    let num_fields_before = bundle.schema().await?.fields().len();

    // Apply filter
    let filtered = bundle
        .filter("salary > $1", vec![ScalarValue::Float64(Some(50000.0))])
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
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Apply filter then remove a column
    let filtered = bundle
        .filter("salary > $1", vec![ScalarValue::Float64(Some(50000.0))])
        .await?;

    let reduced = filtered.remove_column("email").await?;

    // Query should work
    let df = reduced.dataframe().await?;
    let _result = df.as_ref().clone().collect().await?;

    Ok(())
}
#[tokio::test]
async fn test_select_basic() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Select specific columns
    let selected = bundle.select(vec!["id", "first_name", "salary"]).await?;

    // Verify the selection works
    let df = selected.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1);
    assert!(result[0].num_rows() > 0);
    assert_eq!(result[0].num_columns(), 3);

    Ok(())
}
#[tokio::test]
async fn test_select_with_expressions() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Select with expressions and aliases
    let selected = bundle
        .select(vec!["id", "first_name", "salary * 2 AS doubled_salary"])
        .await?;

    let df = selected.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1);
    assert!(result[0].num_rows() > 0);
    assert_eq!(result[0].num_columns(), 3);
    assert_eq!(field_names(&result[0].schema()), vec!["id","first_name","doubled_salary"]);

    Ok(())
}
#[tokio::test]
async fn test_select_with_other_operations() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Chain select with filter
    let filtered = bundle
        .filter("salary > $1", vec![ScalarValue::Float64(Some(50000.0))])
        .await?;
    let selected = filtered.select(vec!["id", "first_name"]).await?;

    let df = selected.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1);
    assert!(result[0].num_rows() > 0);
    assert_eq!(result[0].num_columns(), 2);

    Ok(())
}

#[tokio::test]
async fn test_query_limit() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Query with LIMIT
    let queried = bundle
        .query("SELECT * FROM data LIMIT 10", vec![])
        .await?;

    // Check that the result is actually limited
    let df = queried.dataframe().await?;
    let record_batches = df.as_ref().clone().collect().await?;
    let total_rows: usize = record_batches.iter().map(|rb| rb.num_rows()).sum();

    assert_eq!(
        total_rows, 10,
        "Query with LIMIT 10 should return exactly 10 rows"
    );

    Ok(())
}

#[tokio::test]
async fn test_query_with_filter() -> Result<(), BundlebaseError> {
    let mut bundle =
        bundlebase::BundleBuilder::create(random_memory_url().as_str()).await?;
    bundle.attach(test_datafile("userdata.parquet")).await?;

    // Query with WHERE clause
    let queried = bundle
        .query(
            "SELECT id, salary FROM data WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;

    // Check that columns are correct
    let df = queried.dataframe().await?;
    let record_batches = df.as_ref().clone().collect().await?;

    assert!(!record_batches.is_empty(), "Should have results");
    assert_eq!(record_batches[0].num_columns(), 2, "Should have 2 columns");

    Ok(())
}
