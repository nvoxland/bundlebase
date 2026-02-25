use arrow::array::{Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::{assert_regexp, Bundle, BundlebaseError, IndexType, Operation, TokenizerConfig};
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

    bundle.create_index(&["Email"], IndexType::Column, None).await?;

    let status = bundle.status();
    assert_eq!(1, status.changes().len());
    assert_eq!(
        "CREATE COLUMN INDEX ON Email",
        status.changes()[0].description,
    );

    assert!(
        status.changes()[0]
            .operations
            .iter()
            .map(|op| op.describe())
            .collect::<Vec<_>>()
            .join(", ")
            .contains("CREATE INDEX on column IDs"),
        "Expected operations to contain 'CREATE INDEX on column IDs'"
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
        ops_description.contains("CREATE INDEX on column IDs"),
        "Expected operations to contain 'CREATE INDEX on column IDs', got: {}",
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
    bundle.create_index(&["Email"], IndexType::Column, None).await?;
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
    bundle.create_index(&["Email"], IndexType::Column, None).await?;
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
    bundle.create_index(&["Email"], IndexType::Column, None).await?;
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
    bundle.create_index(&["Customer Id"], IndexType::Column, None).await?;
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
        .create_index(&["Email"], IndexType::Column, None)
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
        .create_index(&["Email"], IndexType::Column, None)
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
        .create_index(&["Email"], IndexType::Column, None)
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
        .create_index(&["Email"], IndexType::Column, None)
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
        .create_index(&["Email"], IndexType::Column, None)
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

#[tokio::test]
async fn test_search_single_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create a named text index on Company
    bundle
        .create_index(&["Company"], IndexType::text(TokenizerConfig::default()), Some("company_search"))
        .await?;
    bundle.commit("Text index created").await?;

    // Query using search() table function
    let stream = bundle
        .query(
            "SELECT \"Index\", \"Company\" FROM search('company_search', 'Group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() should return matching rows, but got 0"
    );

    // Verify every returned row actually contains "Group" in the Company column
    for batch in &rs {
        let companies = batch
            .column_by_name("Company")
            .expect("Company column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Company should be StringArray");

        for i in 0..companies.len() {
            let company = companies.value(i);
            assert!(
                company.to_lowercase().contains("group"),
                "Expected company '{}' to contain 'group'",
                company
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_no_results() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle
        .create_index(&["Company"], IndexType::text(TokenizerConfig::default()), Some("company_search"))
        .await?;
    bundle.commit("Text index created").await?;

    // Query with a term that doesn't match anything
    let stream = bundle
        .query(
            "SELECT \"Index\", \"Company\" FROM search('company_search', 'zzzznonexistent')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert_eq!(
        num_rows, 0,
        "search() with non-matching query should return 0 rows"
    );

    Ok(())
}

#[tokio::test]
async fn test_search_multi_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create a single multi-column text index spanning Company and City
    bundle
        .create_index(
            &["Company", "City"],
            IndexType::text(TokenizerConfig::default()), Some("multi_search"),
        )
        .await?;
    bundle.commit("Multi-column text index created").await?;

    // Field-specific query using tantivy syntax: Company:group
    let stream = bundle
        .query(
            "SELECT \"Company\", \"City\" FROM search('multi_search', 'Company:group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "Field-specific search should return matching rows, but got 0"
    );

    // Verify every returned row contains "group" in Company
    for batch in &rs {
        let companies = batch
            .column_by_name("Company")
            .expect("Company column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("StringArray");

        for i in 0..companies.len() {
            let company = companies.value(i).to_lowercase();
            assert!(
                company.contains("group"),
                "Expected company '{}' to contain 'group'",
                companies.value(i)
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_with_score_ordering() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle
        .create_index(&["Company"], IndexType::text(TokenizerConfig::default()), Some("company_search"))
        .await?;
    bundle.commit("Text index created").await?;

    // Query with _score column and ORDER BY _score DESC
    let stream = bundle
        .query(
            "SELECT \"Company\", _score FROM search('company_search', 'Group') ORDER BY _score DESC",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(num_rows > 0, "search() should return rows with scores");

    // Verify the _score column exists and has positive values
    for batch in &rs {
        let scores = batch
            .column_by_name("_score")
            .expect("_score column should exist");

        let score_array = scores
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .expect("score should be Float64Array");

        for i in 0..score_array.len() {
            assert!(
                score_array.value(i) > 0.0,
                "Score should be positive, got {}",
                score_array.value(i)
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_tantivy_boolean_syntax() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Multi-column index for boolean queries
    bundle
        .create_index(
            &["Company", "City"],
            IndexType::text(TokenizerConfig::default()), Some("multi_search"),
        )
        .await?;
    bundle.commit("Text index created").await?;

    // Boolean AND query using tantivy required-term syntax (+)
    let stream = bundle
        .query(
            "SELECT \"Company\", \"City\" FROM search('multi_search', '+Company:group +City:east')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;

    // Verify every returned row matches BOTH conditions
    for batch in &rs {
        let companies = batch
            .column_by_name("Company")
            .expect("Company column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("StringArray");
        let cities = batch
            .column_by_name("City")
            .expect("City column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("StringArray");

        for i in 0..batch.num_rows() {
            let company = companies.value(i).to_lowercase();
            let city = cities.value(i).to_lowercase();
            assert!(
                company.contains("group") && city.contains("east"),
                "Row {} should match 'group' in Company AND 'east' in City, got Company='{}', City='{}'",
                i, companies.value(i), cities.value(i)
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_with_additional_where() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle
        .create_index(&["Company"], IndexType::text(TokenizerConfig::default()), Some("company_search"))
        .await?;
    bundle.commit("Text index created").await?;

    // search() + additional WHERE filter on a non-indexed column
    let stream = bundle
        .query(
            "SELECT \"Company\", \"City\" FROM search('company_search', 'group') WHERE \"City\" = 'East Leonard'",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;

    // Verify every row matches BOTH conditions
    for batch in &rs {
        let companies = batch
            .column_by_name("Company")
            .expect("Company column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("StringArray");
        let cities = batch
            .column_by_name("City")
            .expect("City column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("StringArray");

        for i in 0..batch.num_rows() {
            let company = companies.value(i).to_lowercase();
            let city = cities.value(i);
            assert!(
                company.contains("group") && city == "East Leonard",
                "Row {} should match 'group' in Company AND City='East Leonard', got Company='{}', City='{}'",
                i, companies.value(i), city
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_single_arg_with_one_text_index() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create a single text index (auto-named)
    bundle
        .create_index(
            &["Company"],
            IndexType::text(TokenizerConfig::default()),
            Some("company_search"),
        )
        .await?;
    bundle.commit("Text index created").await?;

    // Single-arg search — should auto-detect the only text index
    let stream = bundle
        .query(
            "SELECT \"Company\" FROM search('Group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "Single-arg search() should return matching rows, but got 0"
    );

    Ok(())
}

#[tokio::test]
async fn test_search_single_arg_error_with_multiple_text_indexes() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create two text indexes
    bundle
        .create_index(
            &["Company"],
            IndexType::text(TokenizerConfig::default()),
            Some("company_search"),
        )
        .await?;
    bundle
        .create_index(
            &["City"],
            IndexType::text(TokenizerConfig::default()),
            Some("city_search"),
        )
        .await?;
    bundle.commit("Two text indexes created").await?;

    // Single-arg search should error when multiple text indexes exist
    let result = bundle
        .query(
            "SELECT \"Company\" FROM search('Group')",
            vec![],
        )
        .await;

    assert!(
        result.is_err(),
        "Single-arg search() with multiple text indexes should error"
    );

    let err_msg = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        err_msg.contains("2 exist"),
        "Error should mention multiple indexes exist, got: {}",
        err_msg
    );

    Ok(())
}

#[tokio::test]
async fn test_create_index_after_rename_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Rename a column, then create an index on the new name
    bundle.rename_column("City", "city").await?;
    bundle
        .create_index(&["city"], IndexType::Column, None)
        .await?;
    bundle.commit("Index on renamed column").await?;

    // Verify the index works by querying with the new column name
    let stream = bundle
        .query(
            "SELECT \"Index\" FROM bundle WHERE city = 'East Leonard'",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(
        num_rows, 1,
        "Query on renamed+indexed column should return 1 row"
    );

    Ok(())
}

#[tokio::test]
async fn test_create_index_after_standardize_column_names() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // standardize_column_names lowercases and replaces spaces/special chars
    bundle.standardize_column_names().await?;
    bundle
        .create_index(&["city"], IndexType::Column, None)
        .await?;
    bundle.commit("Index after standardize").await?;

    // Verify the index works
    let stream = bundle
        .query(
            "SELECT \"index\" FROM bundle WHERE city = 'East Leonard'",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(
        num_rows, 1,
        "Query on standardized+indexed column should return 1 row"
    );

    Ok(())
}

#[tokio::test]
async fn test_search_after_standardize_column_names() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // standardize_column_names lowercases column names (e.g. "Company" -> "company")
    bundle.standardize_column_names().await?;

    // Create a text index using the standardized (lowercase) column names
    bundle
        .create_index(
            &["company", "city"],
            IndexType::text(TokenizerConfig::default()),
            Some("search_idx"),
        )
        .await?;
    bundle.commit("Text index after standardize").await?;

    // Query using the lowercase column names — this should work because
    // search() now applies rename operations to the physical schema
    let stream = bundle
        .query(
            "SELECT company, city FROM search('search_idx', 'group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() after standardize_column_names should return matching rows"
    );

    // Verify every returned row actually contains "group" in the company column
    for batch in &rs {
        let companies = batch
            .column_by_name("company")
            .expect("company column should exist with standardized name")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("company should be StringArray");

        for i in 0..companies.len() {
            let company = companies.value(i).to_lowercase();
            assert!(
                company.contains("group"),
                "Expected company '{}' to contain 'group'",
                companies.value(i)
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_with_projection_and_where_on_score() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle
        .create_index(
            &["Company", "City"],
            IndexType::text(TokenizerConfig::default()),
            Some("multi_search"),
        )
        .await?;
    bundle.commit("Text index created").await?;

    // Project a subset of columns and filter on _score.
    // This reproduced a bug where partition_statistics used the full output schema
    // while the DataSource emitted the projected schema, causing DataFusion's
    // ExprBoundaries to fail with a column-index-out-of-bounds error.
    let stream = bundle
        .query(
            "SELECT \"Company\", _score FROM search('multi_search', 'group') WHERE _score > 0",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() with projection and WHERE on _score should return rows"
    );

    for batch in &rs {
        let scores = batch
            .column_by_name("_score")
            .expect("_score column should exist")
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .expect("_score should be Float64Array");

        for i in 0..scores.len() {
            assert!(
                scores.value(i) > 0.0,
                "Score should be > 0, got {}",
                scores.value(i)
            );
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_search_field_specific_after_standardize_column_names() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle.standardize_column_names().await?;

    bundle
        .create_index(
            &["company", "city"],
            IndexType::text(TokenizerConfig::default()),
            Some("search_idx"),
        )
        .await?;
    bundle.commit("Text index after standardize").await?;

    // Field-specific query using lowercase (logical) field names
    // These should be rewritten to the physical names (e.g. "Company") before Tantivy sees them
    let stream = bundle
        .query(
            "SELECT company, city FROM search('search_idx', 'company:group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "Field-specific search with standardized column names should return matching rows"
    );

    for batch in &rs {
        let companies = batch
            .column_by_name("company")
            .expect("company column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("company should be StringArray");

        for i in 0..companies.len() {
            let company = companies.value(i).to_lowercase();
            assert!(
                company.contains("group"),
                "Expected company '{}' to contain 'group'",
                companies.value(i)
            );
        }
    }

    Ok(())
}

/// Verify that field-specific text search works after chained renames.
/// E.g., Company → company (standardize) → co (rename) should resolve co → Company for Tantivy.
#[tokio::test]
async fn test_search_after_chained_renames() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle
        .create_index(
            &["Company"],
            IndexType::text(TokenizerConfig::default()),
            Some("search_idx"),
        )
        .await?;

    bundle.standardize_column_names().await?;
    bundle.rename_column("company", "co").await?;
    bundle.commit("Text index with chained renames").await?;

    // Field-specific query using the twice-renamed name "co"
    // Should resolve: co → company → Company (the physical Tantivy field)
    let stream = bundle
        .query(
            "SELECT co FROM search('search_idx', 'co:group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "Field-specific search with chained renames should return matching rows"
    );

    for batch in &rs {
        let companies = batch
            .column_by_name("co")
            .expect("co column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("co should be StringArray");

        for i in 0..companies.len() {
            let company = companies.value(i).to_lowercase();
            assert!(
                company.contains("group"),
                "Expected company '{}' to contain 'group'",
                companies.value(i)
            );
        }
    }

    Ok(())
}

// ==========================================================================
// Cast column + index interaction tests
// ==========================================================================

/// Verify that casting a column and then creating an index on it works correctly.
/// The Index column in the CSV is a numeric string — cast it to integer, index it, query with integer filter.
#[tokio::test]
async fn test_cast_column_then_create_index() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Cast "Index" from string to integer
    bundle.cast_column("Index", "integer", None).await?;

    // Create column index on the cast column
    bundle
        .create_index(&["Index"], IndexType::Column, None)
        .await?;
    bundle.commit("Cast + index").await?;

    // Query with integer filter
    let stream = bundle
        .query(
            "SELECT * FROM bundle WHERE \"Index\" = 1",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(
        1, num_rows,
        "Query on cast+indexed column should return 1 row"
    );

    Ok(())
}

/// Verify that an existing column index still works after casting a *different* column.
#[tokio::test]
async fn test_create_index_then_cast_column_different_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create index on City first
    bundle
        .create_index(&["City"], IndexType::Column, None)
        .await?;
    bundle.commit("Index on City").await?;

    // Cast a different column (Index → integer)
    bundle.cast_column("Index", "integer", None).await?;
    bundle.commit("Cast Index").await?;

    // Verify the City index still works
    let plan = get_physical_plan(
        bundle.as_ref(),
        Some("SELECT * FROM bundle WHERE City = 'East Leonard'"),
    )
    .await?;

    assert!(
        plan.contains("RowIdOffsetDataSource"),
        "Expected City index to still work after casting a different column, plan:\n{}",
        plan
    );

    Ok(())
}

/// Verify that casting a column that already has an index, then reindexing, works correctly.
#[tokio::test]
async fn test_create_index_then_cast_same_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create column index on Index (as string)
    bundle
        .create_index(&["Index"], IndexType::Column, None)
        .await?;
    bundle.commit("Index on string Index").await?;

    // Now cast Index to integer and reindex
    bundle.cast_column("Index", "integer", None).await?;
    bundle.reindex().await?;
    bundle.commit("Cast and reindex").await?;

    // Query with integer filter
    let stream = bundle
        .query(
            "SELECT * FROM bundle WHERE \"Index\" = 1",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(
        1, num_rows,
        "Query on reindexed cast column should return 1 row"
    );

    Ok(())
}

/// Verify that cast_column with a `clean` regex pattern still allows indexing.
/// This tests the Cast(ScalarFunction(Column)) expression chain.
#[tokio::test]
async fn test_cast_column_with_clean_then_create_index() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Cast Index to integer with clean (strip non-digits, though Index is already numeric)
    bundle
        .cast_column("Index", "integer", Some("[^0-9]".to_string()))
        .await?;

    // Create column index on the cleaned+cast column
    bundle
        .create_index(&["Index"], IndexType::Column, None)
        .await?;
    bundle.commit("Cast with clean + index").await?;

    // Query with integer filter
    let stream = bundle
        .query(
            "SELECT * FROM bundle WHERE \"Index\" = 1",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(
        1, num_rows,
        "Query on clean+cast+indexed column should return 1 row"
    );

    Ok(())
}

/// Verify that search() includes columns added via add_column.
#[tokio::test]
async fn test_search_with_add_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create text index on Company
    bundle
        .create_index(
            &["Company"],
            IndexType::text(TokenizerConfig::default()),
            Some("company_search"),
        )
        .await?;

    // Add a computed column
    bundle
        .add_column("company_upper", "upper(\"Company\")")
        .await?;
    bundle.commit("Text index + add_column").await?;

    // Search and select the computed column
    let stream = bundle
        .query(
            "SELECT \"Company\", company_upper, _score FROM search('company_search', 'Group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() with add_column should return matching rows, but got 0"
    );

    // Verify company_upper column exists and contains uppercase values
    for batch in &rs {
        let companies = batch
            .column_by_name("Company")
            .expect("Company column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Company should be StringArray");

        let uppers = batch
            .column_by_name("company_upper")
            .expect("company_upper column should exist in search results")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("company_upper should be StringArray");

        for i in 0..companies.len() {
            assert_eq!(
                uppers.value(i),
                companies.value(i).to_uppercase(),
                "company_upper should be uppercase of Company"
            );
        }
    }

    Ok(())
}

/// Verify that search() reflects cast_column type changes.
#[tokio::test]
async fn test_search_with_cast_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create text index on Company
    bundle
        .create_index(
            &["Company"],
            IndexType::text(TokenizerConfig::default()),
            Some("company_search"),
        )
        .await?;

    // Cast Index column from string to integer
    bundle.cast_column("Index", "integer", None).await?;
    bundle.commit("Text index + cast_column").await?;

    // Search and select the cast column
    let stream = bundle
        .query(
            "SELECT \"Index\", \"Company\", _score FROM search('company_search', 'Group')",
            vec![],
        )
        .await?;

    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() with cast_column should return matching rows, but got 0"
    );

    // Verify Index column is now integer type
    for batch in &rs {
        let index_col = batch
            .column_by_name("Index")
            .expect("Index column should exist in search results");
        assert!(
            index_col.as_any().downcast_ref::<Int64Array>().is_some(),
            "Index should be Int64Array after cast, got {:?}",
            index_col.data_type()
        );
    }

    Ok(())
}

/// Verify that casting one column doesn't break indexing on a different (non-cast) column.
#[tokio::test]
async fn test_cast_column_then_create_index_on_different_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Cast Index to integer
    bundle.cast_column("Index", "integer", None).await?;

    // Create index on City (not the cast column)
    bundle
        .create_index(&["City"], IndexType::Column, None)
        .await?;
    bundle.commit("Cast Index, index City").await?;

    // Verify the City index works
    let plan = get_physical_plan(
        bundle.as_ref(),
        Some("SELECT * FROM bundle WHERE City = 'East Leonard'"),
    )
    .await?;

    assert!(
        plan.contains("RowIdOffsetDataSource"),
        "Expected City index to work after casting a different column, plan:\n{}",
        plan
    );

    Ok(())
}

// ==========================================================================
// Computed column (add_column) + index tests
// ==========================================================================

/// Verify that creating a column index on an add_column computed column works.
#[tokio::test]
async fn test_create_column_index_on_added_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Add a computed column
    bundle
        .add_column("company_upper", "upper(\"Company\")")
        .await?;

    // Create column index on the computed column
    bundle
        .create_index(&["company_upper"], IndexType::Column, None)
        .await?;
    bundle.commit("Index on computed column").await?;

    // Query using the computed column with the index
    let stream = bundle
        .query(
            "SELECT \"Company\", company_upper FROM bundle WHERE company_upper = 'RASMUSSEN GROUP'",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert_eq!(
        num_rows, 1,
        "Query on indexed computed column should return exactly 1 row"
    );

    // Verify the computed column value is correct
    let uppers = rs[0]
        .column_by_name("company_upper")
        .expect("company_upper column should exist")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("company_upper should be StringArray");
    assert_eq!(uppers.value(0), "RASMUSSEN GROUP");

    // Verify the index was built by checking committed operations include INDEX BLOCKS
    let reopened = Bundle::open(data_dir.url().as_str(), None).await?;
    let ops_description = reopened
        .operations()
        .iter()
        .map(|op| op.describe())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        ops_description.contains("INDEX BLOCKS"),
        "Expected operations to contain 'INDEX BLOCKS' for computed column, got: {}",
        ops_description
    );

    Ok(())
}

/// Verify that creating a text index on an add_column computed column works.
#[tokio::test]
async fn test_create_text_index_on_added_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Add a computed column
    bundle
        .add_column("company_upper", "upper(\"Company\")")
        .await?;

    // Create text index on the computed column
    bundle
        .create_index(
            &["company_upper"],
            IndexType::text(TokenizerConfig::default()),
            Some("upper_search"),
        )
        .await?;
    bundle.commit("Text index on computed column").await?;

    // Search using the text index on the computed column
    let stream = bundle
        .query(
            "SELECT \"Company\", company_upper FROM search('upper_search', 'GROUP')",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() on computed column text index should return matching rows, but got 0"
    );

    // Verify every returned row contains "GROUP" in the company_upper column
    for batch in &rs {
        let uppers = batch
            .column_by_name("company_upper")
            .expect("company_upper column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("company_upper should be StringArray");

        for i in 0..uppers.len() {
            assert!(
                uppers.value(i).contains("GROUP"),
                "Expected company_upper '{}' to contain 'GROUP'",
                uppers.value(i)
            );
        }
    }

    Ok(())
}

/// Verify that indexing a computed column works after renaming a source column.
#[tokio::test]
async fn test_create_column_index_on_added_column_after_rename() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Rename Company → company, then add computed column referencing the renamed name
    bundle.rename_column("Company", "company").await?;
    bundle
        .add_column("company_upper", "upper(company)")
        .await?;

    // Create column index on the computed column
    bundle
        .create_index(&["company_upper"], IndexType::Column, None)
        .await?;
    bundle.commit("Index on computed column after rename").await?;

    // Verify the index works
    let stream = bundle
        .query(
            "SELECT company, company_upper FROM bundle WHERE company_upper = 'RASMUSSEN GROUP'",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();
    assert!(
        num_rows > 0,
        "Query on indexed computed column (after rename) should return rows"
    );

    Ok(())
}

/// Verify that search() results include a computed column when the text index is on a physical column.
#[tokio::test]
async fn test_search_with_index_on_added_column() -> Result<(), BundlebaseError> {
    common::enable_logging();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create text index on Company (physical column)
    bundle
        .create_index(
            &["Company"],
            IndexType::text(TokenizerConfig::default()),
            Some("company_search"),
        )
        .await?;

    // Also create a column index on a computed column
    bundle
        .add_column("company_upper", "upper(\"Company\")")
        .await?;
    bundle
        .create_index(&["company_upper"], IndexType::Column, None)
        .await?;
    bundle.commit("Text index + computed column index").await?;

    // Search using the text index and verify the computed column is accessible
    let stream = bundle
        .query(
            "SELECT \"Company\", company_upper FROM search('company_search', 'Group')",
            vec![],
        )
        .await?;
    let rs: Vec<RecordBatch> = stream.try_collect().await?;
    let num_rows: usize = rs.iter().map(|rb| rb.num_rows()).sum();

    assert!(
        num_rows > 0,
        "search() should return rows with computed column accessible"
    );

    // Verify computed column values are correct in search results
    for batch in &rs {
        let companies = batch
            .column_by_name("Company")
            .expect("Company column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Company should be StringArray");

        let uppers = batch
            .column_by_name("company_upper")
            .expect("company_upper column should exist in search results")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("company_upper should be StringArray");

        for i in 0..companies.len() {
            assert_eq!(
                uppers.value(i),
                companies.value(i).to_uppercase(),
                "company_upper should be uppercase of Company in search results"
            );
        }
    }

    Ok(())
}
