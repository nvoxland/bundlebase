use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_command::BundleFacadeCommandExt;
use bundlebase_common::BundlebaseError;
use datafusion::scalar::ScalarValue;
use futures::TryStreamExt;

mod common;

fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        bundlebase_catalog::init();
    });
}

#[tokio::test]
async fn test_query_basic_filter() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Apply SQL query to filter results
    let stream = bundle
        .query(
            "SELECT first_name, last_name FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
            None,
        )
        .await?;

    let record_batches: Vec<_> = stream.try_collect().await?;
    assert!(
        !record_batches.is_empty(),
        "Should have at least one record batch"
    );

    Ok(())
}

#[tokio::test]
async fn test_query_star() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let stream = bundle
        .query("SELECT * FROM bundle LIMIT 10", vec![], None)
        .await?;
    let result: Vec<_> = stream.try_collect().await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].num_rows(), 10);

    Ok(())
}

#[tokio::test]
async fn test_query_lowercase_select() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Lowercase "select" should work
    let stream = bundle
        .query("select * from bundle limit 10", vec![], None)
        .await?;
    let result: Vec<_> = stream.try_collect().await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].num_rows(), 10);

    Ok(())
}

#[tokio::test]
async fn test_query_multiple_parameters() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Apply SQL query with multiple parameters
    let stream = bundle
        .query(
            "SELECT id, first_name FROM bundle WHERE salary > $1 OR gender = $2",
            vec![
                ScalarValue::Float64(Some(100000.0)),
                ScalarValue::Utf8(Some("F".to_string())),
            ],
            None,
        )
        .await?;
    let result: Vec<_> = stream.try_collect().await?;

    assert_eq!(result.len(), 1);
    assert!(
        result[0].num_rows() > 0,
        "Should have results matching either condition"
    );

    Ok(())
}

#[tokio::test]
async fn test_query_no_parameters() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Apply SQL query without parameters
    let stream = bundle
        .query("SELECT * FROM bundle LIMIT 10", vec![], None)
        .await?;
    let result: Vec<_> = stream.try_collect().await?;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].num_rows(), 10);

    Ok(())
}

#[tokio::test]
async fn test_query_with_aggregation() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Apply SQL query with GROUP BY
    let stream = bundle
        .query(
            "SELECT gender, COUNT(*) as count FROM bundle GROUP BY gender",
            vec![],
            None,
        )
        .await?;
    let result: Vec<_> = stream.try_collect().await?;

    assert!(!result.is_empty(), "Should have at least one batch");
    let total_rows: usize = result.iter().map(|batch| batch.num_rows()).sum();
    assert!(total_rows > 0, "Should have aggregation results");

    Ok(())
}

#[tokio::test]
async fn test_explain_basic() -> Result<(), BundlebaseError> {
    init();
    use datafusion::logical_expr::ExplainFormat;
    use futures::StreamExt;

    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Explain should return a stream with plan_type and plan columns
    let mut stream = bundle
        .bundle()
        .explain(false, false, ExplainFormat::Indent, None)
        .await?;
    let mut row_count = 0;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        assert_eq!(
            batch.num_columns(),
            2,
            "Should have plan_type and plan columns"
        );
        row_count += batch.num_rows();
    }
    assert!(row_count > 0, "Explain should return at least one row");

    Ok(())
}

#[tokio::test]
async fn test_explain_with_filter() -> Result<(), BundlebaseError> {
    init();
    use datafusion::logical_expr::ExplainFormat;
    use futures::StreamExt;

    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Apply a filter and explain
    let filtered = bundle
        .filter(
            "SELECT * FROM bundle WHERE salary > $1",
            vec![ScalarValue::Float64(Some(50000.0))],
        )
        .await?;
    let mut stream = filtered
        .bundle()
        .explain(false, false, ExplainFormat::Indent, None)
        .await?;

    let mut row_count = 0;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        row_count += batch.num_rows();
    }
    assert!(
        row_count > 0,
        "Explain should produce at least one row for filtered bundle"
    );

    Ok(())
}

#[tokio::test]
async fn test_query_table_alias_qualified_wildcard() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let stream = bundle
        .query("SELECT t.* FROM bundle t", vec![], None)
        .await?;
    let result: Vec<_> = stream.try_collect().await?;

    assert_eq!(result.len(), 1);
    assert!(result[0].num_rows() > 0);

    Ok(())
}

#[tokio::test]
async fn test_query_empty_bundle() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let stream = bundle.query("SELECT * FROM bundle", vec![], None).await?;
    let schema = stream.schema().clone();
    let result: Vec<_> = stream.try_collect().await?;

    // Should have exactly one column: no_data (not duplicated)
    assert_eq!(
        schema.fields().len(),
        1,
        "Empty bundle should have exactly 1 column, not duplicated"
    );
    assert_eq!(schema.field(0).name(), "no_data");

    let total_rows: usize = result.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 0, "Empty bundle should have 0 rows");

    Ok(())
}
