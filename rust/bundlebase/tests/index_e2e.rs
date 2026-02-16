use arrow::array::StringArray;
use arrow::record_batch::RecordBatch;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::{assert_regexp, Bundle, BundlebaseError, IndexType, Operation};
use datafusion::common::ScalarValue;
use datafusion::logical_expr::ExplainFormat;
use futures::{StreamExt, TryStreamExt};

mod common;

/// Helper to collect the physical plan text from an explain stream.
async fn get_physical_plan(
    bundle: &dyn BundleFacade,
    sql: Option<&str>,
) -> Result<String, BundlebaseError> {
    let mut stream = bundle
        .explain(false, false, ExplainFormat::Indent, sql)
        .await?;
    let mut plans = Vec::new();
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        let plan_types = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("plan_type column should be StringArray");
        let plan_texts = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("plan column should be StringArray");
        for i in 0..batch.num_rows() {
            if plan_types.value(i) == "physical_plan" {
                plans.push(plan_texts.value(i).to_string());
            }
        }
    }
    Ok(plans.join("\n"))
}

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

// ==========================================================================
// Index usage verification tests
//
// These tests use EXPLAIN to inspect the physical plan and verify that
// index-based query acceleration (RowIdOffsetDataSource) is used when an
// index exists, and that a regular scan is used when it doesn't.
// ==========================================================================

/// Verify that the query path (bundle.query via DefaultSchemaProvider) uses the
/// index when one exists. The physical plan should contain RowIdOffsetDataSource.
#[tokio::test]
async fn test_query_path_uses_index() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .create_index("Email", IndexType::Column)
        .await?;
    bundle.commit("Index on Email").await?;

    // Explain a query with a WHERE clause on the indexed column.
    // This exercises DefaultSchemaProvider → BundleViewTable → PackTable → DataBlock.
    let plan = get_physical_plan(
        bundle.as_ref(),
        Some("SELECT * FROM bundle WHERE Email = 'elizabethbarr@ewing.com'"),
    )
    .await?;

    assert!(
        plan.contains("RowIdOffsetDataSource"),
        "Expected physical plan to use RowIdOffsetDataSource (index lookup) but got:\n{}",
        plan
    );

    Ok(())
}

/// Verify that without an index, the physical plan does NOT contain
/// RowIdOffsetDataSource (uses a full scan instead).
#[tokio::test]
async fn test_query_path_without_index_uses_full_scan() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle.commit("No index").await?;

    let plan = get_physical_plan(
        bundle.as_ref(),
        Some("SELECT * FROM bundle WHERE Email = 'elizabethbarr@ewing.com'"),
    )
    .await?;

    assert!(
        !plan.contains("RowIdOffsetDataSource"),
        "Expected full scan (no RowIdOffsetDataSource) but got:\n{}",
        plan
    );

    Ok(())
}

/// Verify that the filter path (bundle.filter via FilterOp) also results in
/// index usage visible in the subsequent explain.
#[tokio::test]
async fn test_filter_path_uses_index() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .create_index("Email", IndexType::Column)
        .await?;
    bundle.commit("Index on Email").await?;

    // Apply a filter through the FilterOp path (exercises FilterOp → BundleViewTable)
    bundle
        .filter(
            "SELECT * FROM bundle WHERE Email = 'elizabethbarr@ewing.com'",
            vec![],
        )
        .await?;

    // Explain the filtered bundle's current plan (no SQL argument)
    let plan = get_physical_plan(bundle.as_ref(), None).await?;

    assert!(
        plan.contains("RowIdOffsetDataSource"),
        "Expected physical plan to use RowIdOffsetDataSource after filter but got:\n{}",
        plan
    );

    Ok(())
}

/// Verify that querying on a non-indexed column still performs a full scan
/// even when other columns are indexed.
#[tokio::test]
async fn test_query_on_non_indexed_column_uses_full_scan() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .create_index("Email", IndexType::Column)
        .await?;
    bundle.commit("Index on Email only").await?;

    // Query on City (not indexed) — should NOT use RowIdOffsetDataSource
    let plan = get_physical_plan(
        bundle.as_ref(),
        Some("SELECT * FROM bundle WHERE City = 'East Leonard'"),
    )
    .await?;

    assert!(
        !plan.contains("RowIdOffsetDataSource"),
        "Expected full scan for non-indexed column but got:\n{}",
        plan
    );

    Ok(())
}

/// Verify that filters are pushed down through BundleViewTable by checking
/// the logical plan for partial_filters on the table scan node.
#[tokio::test]
async fn test_filters_pushed_down_to_table_scan() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle.commit("Setup").await?;

    // Get the full explain output (both logical and physical plans)
    let mut stream = bundle
        .as_ref()
        .explain(
            false,
            false,
            ExplainFormat::Indent,
            Some("SELECT * FROM bundle WHERE Email = 'test@example.com'"),
        )
        .await?;

    let mut logical_plan = String::new();
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        let plan_types = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("plan_type column");
        let plan_texts = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("plan column");
        for i in 0..batch.num_rows() {
            if plan_types.value(i) == "logical_plan" {
                logical_plan.push_str(plan_texts.value(i));
            }
        }
    }

    // The logical plan should show partial_filters on the TableScan,
    // proving that filters were pushed down through BundleViewTable
    assert!(
        logical_plan.contains("partial_filters"),
        "Expected logical plan to contain partial_filters (filter pushdown) but got:\n{}",
        logical_plan
    );

    Ok(())
}

/// Verify index usage via the query path with a parameterized filter.
#[tokio::test]
async fn test_query_path_index_with_parameterized_filter() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .create_index("Email", IndexType::Column)
        .await?;
    bundle.commit("Index on Email").await?;

    // Use a parameterized query with $1
    let stream = bundle
        .query(
            "SELECT * FROM bundle WHERE Email = $1",
            vec![ScalarValue::Utf8(Some(
                "elizabethbarr@ewing.com".to_string(),
            ))],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(1, num_rows, "Expected exactly 1 match for parameterized query");

    Ok(())
}

/// Verify that after reopening a committed bundle, index acceleration still works.
#[tokio::test]
async fn test_index_survives_reopen() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .create_index("Email", IndexType::Column)
        .await?;
    bundle.commit("Index on Email").await?;

    // Reopen the bundle from disk
    let reopened = Bundle::open(data_dir.url().as_str(), None).await?;

    let plan = get_physical_plan(
        reopened.as_ref(),
        Some("SELECT * FROM bundle WHERE Email = 'elizabethbarr@ewing.com'"),
    )
    .await?;

    assert!(
        plan.contains("RowIdOffsetDataSource"),
        "Expected index to survive reopen but physical plan was:\n{}",
        plan
    );

    Ok(())
}
