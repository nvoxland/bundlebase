use bundlebase::{BundlebaseError, Bundle, Operation};
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use arrow::record_batch::RecordBatch;
use datafusion::common::ScalarValue;

mod common;

#[tokio::test]
async fn test_basic_indexing() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(data_dir.url().as_str()).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;

    bundle.index("Email").await?;

    assert_eq!(2, bundle.status().changes().len());
    assert_eq!("Attach memory:///test_data/customers-0-100.csv", bundle.status().changes()[0].description);
    assert_eq!("Index column Email", bundle.status().changes()[1].description);

    assert_eq!("DEFINE INDEX on Email, INDEX BLOCKS", bundle.status().changes()[1].operations.iter().map(|op| op.describe()).collect::<Vec<_>>().join(", "));

    bundle.commit("Created index").await?;

    let bundle_loaded = Bundle::open(data_dir.url().as_str()).await?;
    let ops_description = bundle_loaded.operations().iter().map(|op| op.describe()).collect::<Vec<_>>().join(", ");
    assert!(ops_description.contains("DEFINE INDEX on Email"), "Expected operations to contain 'DEFINE INDEX on Email', got: {}", ops_description);
    assert!(ops_description.contains("INDEX BLOCKS"), "Expected operations to contain 'INDEX BLOCKS', got: {}", ops_description);


    Ok(())
}

#[tokio::test]
async fn test_query_with_indexed_column_exact_match() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(data_dir.url().as_str()).await?;

    // Attach CSV data
    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;

    // Create index on Email column
    bundle.index("Email").await?;
    bundle.commit("Created index on Email").await?;

    // Query with exact match on indexed column
    // This should use the index internally
    bundle
        .filter("Email = $1", vec![ScalarValue::Utf8(Some("zunigavanessa@smith.info".to_string()))])
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Verify we got exactly one row
    assert_eq!(1, result.len());
    assert_eq!(1, result[0].num_rows());

    // Verify the Email column exists (proving we got data, not an error)
    assert!(result[0].column_by_name("Email").is_some());

    Ok(())
}

#[tokio::test]
async fn test_query_with_indexed_column_in_list() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(data_dir.url().as_str()).await?;

    // Attach CSV data
    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;

    // Create index on Email column
    bundle.index("Email").await?;
    bundle.commit("Created index on Email").await?;

    // Query with IN list on indexed column
    bundle
        .filter("Email IN ($1, $2)", vec![
            ScalarValue::Utf8(Some("zunigavanessa@smith.info".to_string())),
            ScalarValue::Utf8(Some("nonexistent@example.com".to_string())),
        ])
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Verify we got exactly one row (only the first email exists)
    assert_eq!(1, result.len());
    assert_eq!(1, result[0].num_rows());

    Ok(())
}

#[tokio::test]
async fn test_query_without_index_falls_back() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(data_dir.url().as_str()).await?;

    // Attach CSV data but DON'T create index
    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;

    bundle.commit("Attached data without index").await?;

    // Query should still work, just without index optimization
    bundle
        .filter("Email = $1", vec![ScalarValue::Utf8(Some("zunigavanessa@smith.info".to_string()))])
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Verify we still get the correct result via full scan
    assert_eq!(1, result.len());
    assert_eq!(1, result[0].num_rows());

    Ok(())
}

#[tokio::test]
async fn test_query_on_non_indexed_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(data_dir.url().as_str()).await?;

    // Attach CSV data
    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;

    // Create index on Email but query on City (not indexed)
    bundle.index("Email").await?;
    bundle.commit("Created index on Email").await?;

    // Query on non-indexed column should fall back to full scan
    bundle
        .filter("City = $1", vec![ScalarValue::Utf8(Some("East Leonard".to_string()))])
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Verify we still get results via full scan
    assert_eq!(1, result.len());
    assert!(result[0].num_rows() >= 1);

    Ok(())
}

#[tokio::test]
async fn test_index_selectivity() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(data_dir.url().as_str()).await?;

    // Attach CSV data
    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;

    // Create index on Customer Id (should be unique)
    bundle.index("Customer Id").await?;
    bundle.commit("Created index on Customer Id").await?;

    // Query for specific customer
    bundle
        .filter("\"Customer Id\" = $1", vec![ScalarValue::Utf8(Some("DD37Cf93aecA6Dc".to_string()))])
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Should find exactly one customer
    assert_eq!(1, result.len());
    assert_eq!(1, result[0].num_rows());

    // Verify the Customer Id column exists (proving we got data, not an error)
    assert!(result[0].column_by_name("Customer Id").is_some());
    assert!(result[0].column_by_name("First Name").is_some());

    Ok(())
}