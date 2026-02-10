use arrow::array::record_batch;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::functions::{FunctionSignature, StaticImpl};
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use std::sync::Arc;

mod common;

#[tokio::test]
async fn test_schema_tracking_through_operations() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Initially has only the sentinel no_data column
    assert_eq!(1, bundle.schema().await?.fields().len());
    assert_eq!(bundle.schema().await?.field(0).name(), "no_data");

    // After attach
    bundle.attach(test_datafile("userdata.parquet"), None).await?;
    assert_eq!(13, bundle.schema().await?.fields().len());
    assert!(bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "first_name"));
    assert!(bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "title"));

    // After remove
    bundle.drop_column("title").await?;
    assert_eq!(12, bundle.schema().await?.fields().len());
    assert!(!bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "title"));
    assert!(bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "first_name"));

    // After rename
    bundle.rename_column("first_name", "given_name").await?;
    assert_eq!(12, bundle.schema().await?.fields().len());
    assert!(!bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "first_name"));
    assert!(bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "given_name"));

    Ok(())
}
#[tokio::test]
async fn test_schema_consistency_with_dataframe() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;
    bundle.drop_column("title").await?;
    bundle.rename_column("first_name", "given_name").await?;

    // Schema from bundle should match DataFrame schema
    let schema = bundle.schema().await?;
    let bundle_cols: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    let df = bundle.dataframe().await?;
    let df_cols: Vec<String> = df.schema().field_names();

    assert_eq!(
        bundle_cols.len(),
        df_cols.len(),
        "Column count mismatch: bundle has {}, DataFrame has {}",
        bundle_cols.len(),
        df_cols.len()
    );
    for col in &bundle_cols {
        // Handle both qualified and unqualified column names (DataFrame may have ?table?.column_name format)
        let found = df_cols
            .iter()
            .any(|c| c.split('.').last().unwrap_or(c) == *col || c == *col);
        assert!(found, "Column {} not found in DataFrame schema", col);
    }

    Ok(())
}
#[tokio::test]
async fn test_schema_types_preserved() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Get original type of 'id' column
    let schema = bundle.schema().await?;
    let original_type = schema
        .fields()
        .iter()
        .find(|f| f.name() == "id")
        .map(|f| f.data_type())
        .unwrap()
        .clone();

    // After rename, type should be preserved
    bundle.rename_column("id", "user_id").await?;
    let schema = bundle.schema().await?;
    let renamed_type = schema
        .fields()
        .iter()
        .find(|f| f.name() == "user_id")
        .map(|f| f.data_type())
        .unwrap()
        .clone();

    assert_eq!(original_type, renamed_type);

    Ok(())
}
#[tokio::test]
async fn test_remove_all_columns() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Create a simple function with just 2 columns
    bundle
        .create_function(FunctionSignature::new(
            "simple",
            SchemaRef::new(Schema::new(vec![
                Field::new("col1", DataType::Int32, false),
                Field::new("col2", DataType::Utf8, true),
            ])),
        ))
        .await?;
    bundle
        .set_impl(
            "simple",
            Arc::new(StaticImpl::new(
                vec![record_batch!(
                    ("col1", Int32, [1, 2]),
                    ("col2", Utf8, ["a", "b"])
                )?],
                "v1".to_string(),
            )),
        )
        .await?;
    bundle.attach("function://simple", None).await?;

    // Remove both columns
    bundle.drop_column("col1").await?;
    bundle.drop_column("col2").await?;

    // Schema should be empty
    assert_eq!(0, bundle.schema().await?.fields().len());

    // DataFrame should have 0 columns but preserve row count
    let df = bundle.dataframe().await?;
    assert_eq!(0, df.schema().fields().len());

    Ok(())
}
#[tokio::test]
async fn test_rename_with_unicode() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Rename to unicode column name
    bundle.rename_column("first_name", "名前").await?;

    assert!(bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "名前"));
    assert!(!bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "first_name"));

    // Verify in DataFrame
    let df = bundle.dataframe().await?;
    assert!(df.schema().field_with_unqualified_name("名前").is_ok());

    Ok(())
}
#[tokio::test]
async fn test_column_with_special_characters() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Rename to column name with special characters
    bundle
        .rename_column("first_name", "user.first-name@2024")
        .await?;

    assert!(bundle
        .schema()
        .await?
        .fields()
        .iter()
        .any(|f| f.name() == "user.first-name@2024"));

    Ok(())
}
