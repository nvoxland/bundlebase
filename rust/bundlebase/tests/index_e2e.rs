use arrow::record_batch::RecordBatch;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::{assert_regexp, Bundle, BundlebaseError, IndexType, Operation};
use datafusion::common::ScalarValue;
use futures::TryStreamExt;

mod common;

#[tokio::test]
async fn test_basic_indexing() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;
    bundle.commit("No index").await?;

    // Query without index
    let stream = bundle
        .query(
            "select Index, City from bundle where Email='elizabethbarr@ewing.com'",
            vec![],
        )
        .await?;
    let rs: Vec<_> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(1, num_rows, "Query should return 1 row matching the email");

    //todo: support explain passing a query
//     let explain = bundle.explain().await?;
//     assert_regexp!(
//         r#"
// \*\*\* logical_plan \*\*\*
// Projection: packs.__pack_\w\w.Index, packs.__pack_\w\w.City
//   Filter: packs.__pack_\w\w.Email = Utf8\("elizabethbarr@ewing.com"\)
//     TableScan: packs.__pack_\w\w projection=\[Index, City, Email], partial_filters=\[packs.__pack_\w\w.Email = Utf8\("elizabethbarr@ewing.com"\)]
//
// \*\*\* physical_plan \*\*\*
// FilterExec: Email@\d+ = elizabethbarr@ewing.com, projection=\[Index@\d+, City@\d+\]
//   RepartitionExec: partitioning=RoundRobinBatch\(\d+\), input_partitions=1
//     DataSourceExec: file_groups=\{1 group: \[\[test_data/customers-0-100.csv\]\]\}, projection=\[Index, City, Email\], file_type=csv, has_header=true
// "#,
//         explain
//     );

    bundle.create_index("Email", IndexType::Column).await?;

    let status = bundle.status();
    assert_eq!(1, status.changes().len());
    assert_eq!(
        "CREATE INDEX ON Email",
        status.changes()[0].description
    );

    assert_eq!(
        "CREATE INDEX on Email, INDEX BLOCKS",
        status.changes()[0]
            .operations
            .iter()
            .map(|op| op.describe())
            .collect::<Vec<_>>()
            .join(", ")
    );

    bundle.commit("Created index").await?;

    let bundle_loaded = Bundle::open(data_dir.url().as_str(), None).await?;
    let ops_description = bundle_loaded
        .operations()
        .iter()
        .map(|op| op.describe())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        ops_description.contains("CREATE INDEX on Email"),
        "Expected operations to contain 'CREATE INDEX on Email', got: {}",
        ops_description
    );
    assert!(
        ops_description.contains("INDEX BLOCKS"),
        "Expected operations to contain 'INDEX BLOCKS', got: {}",
        ops_description
    );

    // Query with index - should still return correct results
    let stream = bundle
        .query(
            "select Index, City from bundle where Email='elizabethbarr@ewing.com'",
            vec![],
        )
        .await?;
    let rs: Vec<_> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(1, num_rows, "Query with index should return 1 row matching the email");

    //todo explain query
//       let explain = rs.bundle().explain().await?;
//     assert_regexp!(
//         r#"
// \*\*\* logical_plan \*\*\*
// Projection: packs.__pack_\w\w.Index, packs.__pack_\w\w.City
//   Filter: packs.__pack_\w\w.Email = Utf8\("elizabethbarr@ewing.com"\)
//     TableScan: packs.__pack_\w\w projection=\[Index, City, Email\], partial_filters=\[packs.__pack_\w\w.Email = Utf8\("elizabethbarr@ewing.com"\)\]
//
// \*\*\* physical_plan \*\*\*
// FilterExec: Email@2 = elizabethbarr@ewing.com, projection=\[Index@0, City@1\]
//   CooperativeExec
//     DataSourceExec: RowIdOffsetDataSource\[file=memory:///test_data/customers-0-100.csv, rows=1, format=Csv\]
// "#,

    Ok(())
}

#[tokio::test]
async fn test_select_with_indexed_column_exact_match() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    // Attach CSV data
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    // Create index on Email column
    bundle.create_index("Email", IndexType::Column).await?;
    bundle.commit("Created index on Email").await?;

    // Query with exact match on indexed column
    // This should use the index internally
    bundle
        .filter(
            "SELECT * FROM bundle WHERE Email = $1",
            vec![ScalarValue::Utf8(Some(
                "zunigavanessa@smith.info".to_string(),
            ))],
        )
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
async fn test_select_with_indexed_column_in_list() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    // Attach CSV data
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    // Create index on Email column
    bundle.create_index("Email", IndexType::Column).await?;
    bundle.commit("Created index on Email").await?;

    // Query with IN list on indexed column
    bundle
        .filter(
            "SELECT * FROM bundle WHERE Email IN ($1, $2)",
            vec![
                ScalarValue::Utf8(Some("zunigavanessa@smith.info".to_string())),
                ScalarValue::Utf8(Some("nonexistent@example.com".to_string())),
            ],
        )
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Verify we got exactly one row (only the first email exists)
    assert_eq!(1, result.len());
    assert_eq!(1, result[0].num_rows());

    Ok(())
}

#[tokio::test]
async fn test_select_without_index_falls_back() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    // Attach CSV data but DON'T create index
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    bundle.commit("Attached data without index").await?;

    // Query should still work, just without index optimization
    bundle
        .filter(
            "SELECT * FROM bundle WHERE Email = $1",
            vec![ScalarValue::Utf8(Some(
                "zunigavanessa@smith.info".to_string(),
            ))],
        )
        .await?;

    let df = bundle.dataframe().await?;
    let result: Vec<RecordBatch> = df.as_ref().clone().collect().await?;

    // Verify we still get the correct result via full scan
    assert_eq!(1, result.len());
    assert_eq!(1, result[0].num_rows());

    Ok(())
}

#[tokio::test]
async fn test_select_on_non_indexed_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    // Attach CSV data
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    // Create index on Email but query on City (not indexed)
    bundle.create_index("Email", IndexType::Column).await?;
    bundle.commit("Created index on Email").await?;

    // Query on non-indexed column should fall back to full scan
    bundle
        .filter(
            "SELECT * FROM bundle WHERE City = $1",
            vec![ScalarValue::Utf8(Some("East Leonard".to_string()))],
        )
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
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    // Attach CSV data
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    // Create index on Customer Id (should be unique)
    bundle.create_index("Customer Id", IndexType::Column).await?;
    bundle.commit("Created index on Customer Id").await?;

    // Query for specific customer
    bundle
        .filter(
            "SELECT * FROM bundle WHERE \"Customer Id\" = $1",
            vec![ScalarValue::Utf8(Some("DD37Cf93aecA6Dc".to_string()))],
        )
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
