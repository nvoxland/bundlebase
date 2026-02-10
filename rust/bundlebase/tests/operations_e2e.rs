use arrow::array::record_batch;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::functions::{FunctionSignature, StaticImpl};
use bundlebase::test_utils::{field_names, random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use std::sync::Arc;
use url::Url;

mod common;

#[tokio::test]
async fn test_create() -> Result<(), BundlebaseError> {
    let bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    assert_eq!(0, bundle.num_rows().await?);
    let schema = bundle.dataframe().await?.schema().clone();
    assert_eq!(schema.columns().len(), 1, "Empty bundle should have sentinel no_data column");
    assert_eq!(schema.field(0).name(), "no_data");

    Ok(())
}
#[tokio::test]
async fn test_attach() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    let full_path = test_datafile("userdata.parquet");
    bundle.attach(&full_path, None).await?;

    assert_eq!(1000, bundle.bundle().num_rows().await?);

    let df = bundle.dataframe().await?;
    for batch in df.as_ref().clone().collect().await? {
        assert_eq!(1000, batch.num_rows())
    }

    assert_eq!(
        vec![
            "registration_dttm",
            "id",
            "first_name",
            "last_name",
            "email",
            "gender",
            "ip_address",
            "cc",
            "country",
            "birthdate",
            "salary",
            "title",
            "comments"
        ],
        bundle
            .schema()
            .await?
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect::<Vec<_>>()
    );

    Ok(())
}
#[tokio::test]
async fn test_remove() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;
    bundle.drop_column("title").await?;

    assert!(!bundle
        .bundle()
        .dataframe()
        .await?
        .schema()
        .has_column_with_unqualified_name("title"));

    Ok(())
}
#[tokio::test]
async fn test_rename() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;
    bundle.rename_column("first_name", "new_name").await?;

    assert_eq!(
        vec![
            "registration_dttm",
            "id",
            "new_name",
            "last_name",
            "email",
            "gender",
            "ip_address",
            "cc",
            "country",
            "birthdate",
            "salary",
            "title",
            "comments"
        ],
        field_names(&bundle.schema().await?)
    );

    bundle.rename_column("new_name", "newer_name").await?;

    assert_eq!(
        vec![
            "registration_dttm",
            "id",
            "newer_name",
            "last_name",
            "email",
            "gender",
            "ip_address",
            "cc",
            "country",
            "birthdate",
            "salary",
            "title",
            "comments"
        ],
        field_names(&bundle.schema().await?)
    );
    Ok(())
}

#[tokio::test]
async fn test_rename_case_sensitive() -> Result<(), BundlebaseError> {
    let _ = env_logger::builder().is_test(true).try_init();

    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;
    bundle.rename_column("Email", "email").await?;

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
            "email",
            "Subscription Date",
            "Website"
        ],
        field_names(&bundle.schema().await?)
    );

    bundle.rename_column("email", "e-mail").await?;

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
            "e-mail",
            "Subscription Date",
            "Website"
        ],
        field_names(&bundle.schema().await?)
    );
    Ok(())
}

#[tokio::test]
async fn test_function_source() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle
        .create_function(FunctionSignature::new(
            "names",
            SchemaRef::new(Schema::new(vec![Field::new("name", DataType::Utf8, false)])),
        ))
        .await?;
    bundle
        .set_impl(
            "names",
            Arc::new(StaticImpl::new(
                vec![record_batch!(("name", Utf8, ["Alice", "Bob", "Charlie"]))?],
                "test_v1".to_string(),
            )),
        )
        .await?;
    bundle.attach("function://names", None).await?;
    assert_eq!(3, bundle.num_rows().await?);

    Ok(())
}
#[tokio::test]
async fn test_multi_operation_pipeline() -> Result<(), BundlebaseError> {
    // Test a realistic workflow: attach -> remove -> rename -> query
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;

    // Remove multiple columns
    bundle.drop_column("title").await?;
    bundle.drop_column("comments").await?;

    // Rename a column
    bundle.rename_column("first_name", "given_name").await?;

    // Verify final schema
    let schema = &bundle.schema().await?;
    let schema_keys: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(schema_keys.contains(&"given_name"));
    assert!(!schema_keys.contains(&"first_name"));
    assert!(!schema_keys.contains(&"title"));
    assert!(!schema_keys.contains(&"comments"));

    // Verify data integrity
    assert_eq!(1000, bundle.num_rows().await?);

    Ok(())
}
#[tokio::test]
async fn test_sequential_renames() -> Result<(), BundlebaseError> {
    // Test multiple renames in sequence
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;
    bundle.rename_column("first_name", "fname").await?;
    bundle.rename_column("last_name", "lname").await?;
    bundle.rename_column("email", "email_addr").await?;

    let schema = bundle.schema().await?;
    let schema_keys: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert!(schema_keys.contains(&"fname"));
    assert!(schema_keys.contains(&"lname"));
    assert!(schema_keys.contains(&"email_addr"));
    assert!(!schema_keys.contains(&"first_name"));
    assert!(!schema_keys.contains(&"last_name"));
    assert!(!schema_keys.contains(&"email"));

    Ok(())
}
#[tokio::test]
async fn test_attach_missing_file_error() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Should fail when attaching a file that doesn't exist
    let nonexistent_path =
        std::env::current_dir()?.join("../../test_data/nonexistent_file.parquet");
    let nonexistent_url = Url::from_file_path(nonexistent_path).unwrap();
    let result = bundle.attach(nonexistent_url.as_str(), None).await;
    assert!(result.is_err());

    Ok(())
}
#[tokio::test]
async fn test_attach_invalid_function_error() -> Result<(), BundlebaseError> {
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Should fail when attaching a function that hasn't been defined
    let result = bundle.attach("function://undefined_function", None).await;
    assert!(result.is_err());

    Ok(())
}
