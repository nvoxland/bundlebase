use bundlebase;
use bundlebase::bundle::{BundleFacade, JoinTypeOption};
use bundlebase::test_utils::{field_names, random_memory_url, test_datafile};
use bundlebase::BundlebaseError;

mod common;

#[tokio::test]
async fn test_join_basic() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("customers-0-100.csv")).await?;

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
            test_datafile("sales-regions.csv"),
            r#"$base."Country" = regions."Country""#,
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
            "Country",
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
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("customers-0-100.csv")).await?;

    // Join with sales regions on Country
    let bundle = bundle
        .join(
            "regions",
            test_datafile("sales-regions.csv"),
            r#"$base."Country" = regions."Country""#,
            JoinTypeOption::Inner,
        )
        .await?;

    // Try to query the joined data
    assert_eq!(99, bundle.num_rows().await?);

    bundle
        .attach_to_join("regions", test_datafile("sales-regions-2.csv"))
        .await?;
    assert_eq!(100, bundle.bundle().num_rows().await?);

    Ok(())
}

#[tokio::test]
async fn test_join_with_left_join_type() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("customers-0-100.csv")).await?;

    // Join with a left join
    let bundle = bundle
        .join(
            "regions",
            test_datafile("sales-regions.csv"),
            r#"$base."Country" = regions."Country""#,
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
