use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::bundle::JoinTypeOption;
use bundlebase::test_utils::{field_names, random_memory_url, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;

use arrow::array::StringArray;
use futures::TryStreamExt;

mod common;

fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        bundlebase_catalog::init();
    });
}

#[tokio::test]
async fn test_join_basic() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Get schema before join
    let schema_before = &bundle.schema().await?;
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
        field_names(schema_before)
    );

    // Join with sales regions on Country
    let bundle = bundle
        .join(
            "regions",
            r#"bundle."Country" = regions."Country""#,
            Some(test_datafile("sales-regions.csv")),
            JoinTypeOption::Inner,
        )
        .await?;

    let schema_after = &bundle.schema().await?;
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
            "Website",
            "regions_Country",
            "Sales Region",
            "Region Manager"
        ],
        field_names(schema_after)
    );
    // Try to query the joined data
    assert_eq!(99, bundle.num_rows().await?);

    Ok(())
}

#[tokio::test]
async fn test_join_appending() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Join with sales regions on Country
    let bundle = bundle
        .join(
            "regions",
            r#"bundle."Country" = regions."Country""#,
            Some(test_datafile("sales-regions.csv")),
            JoinTypeOption::Inner,
        )
        .await?;

    // Try to query the joined data
    assert_eq!(99, bundle.num_rows().await?);

    bundle
        .attach(test_datafile("sales-regions-2.csv"), Some("regions"))
        .await?;
    assert_eq!(100, bundle.bundle().num_rows().await?);

    Ok(())
}

#[tokio::test]
async fn test_join_with_left_join_type() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Join with a left join
    let bundle = bundle
        .join(
            "regions",
            r#"bundle."Country" = regions."Country""#,
            Some(test_datafile("sales-regions.csv")),
            JoinTypeOption::Left,
        )
        .await?;

    // Try to query
    let df = bundle.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    println!("Left join successful, got {} batches", result.len());
    assert!(!result.is_empty(), "Should have at least one record batch");

    Ok(())
}

#[tokio::test]
async fn test_join_without_url_then_attach() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Create join point without any initial data
    let bundle = bundle
        .join(
            "regions", // No URL
            r#"bundle."Country" = regions."Country""#,
            None,
            JoinTypeOption::Inner,
        )
        .await?;

    // Now attach data to the join
    bundle
        .attach(test_datafile("sales-regions.csv"), Some("regions"))
        .await?;

    // Query should now work with matched data
    let num_rows = bundle.bundle().num_rows().await?;
    assert_eq!(99, num_rows); // Inner join filters out unmatched

    Ok(())
}

#[tokio::test]
async fn test_join_resolves_renamed_columns() -> Result<(), BundlebaseError> {
    init();
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    // Rename "Country" to "country_name" before joining
    bundle.rename_column("Country", "country_name").await?;

    // JOIN using the renamed column name
    let bundle = bundle
        .join(
            "regions",
            r#"bundle.country_name = regions."Country""#,
            Some(test_datafile("sales-regions.csv")),
            JoinTypeOption::Inner,
        )
        .await?;

    // Verify the join worked and the renamed column is in the schema
    let schema = bundle.schema().await?;
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(
        names.contains(&"country_name"),
        "Should contain renamed column"
    );
    assert!(
        names.contains(&"Sales Region"),
        "Should contain joined column"
    );

    // Verify we get correct number of rows (same as un-renamed join)
    assert_eq!(99, bundle.num_rows().await?);

    // Verify we can query using the renamed column
    let stream = bundle
        .query(
            r#"SELECT country_name, "Sales Region" FROM bundle LIMIT 1"#,
            vec![],
            None,
        )
        .await?;
    let batches: Vec<_> = stream.try_collect().await?;
    assert!(!batches.is_empty());
    let batch = &batches[0];
    assert_eq!(batch.num_columns(), 2);

    // Verify both columns have data
    let country_col = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert!(!country_col.value(0).is_empty());

    Ok(())
}
