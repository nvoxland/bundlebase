use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::BundleBuilder;

#[tokio::test]
async fn test_bundle_data_table() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Populate cache by attaching data and getting the dataframe
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    let df = bundle.dataframe().await.unwrap();

    // Debug: Check if cache is populated
    let df_fields = df.schema().fields().len();
    println!("DataFrame schema has {} fields", df_fields);
    assert!(df_fields > 0, "DataFrame should have fields");

    // Query via ctx - should return the cached dataframe
    let result = bundle.bundle.ctx().sql("SELECT * FROM bundle").await.unwrap();

    // Verify it works
    let schema = result.schema();
    println!("Result schema has {} fields", schema.fields().len());
    assert!(schema.fields().len() > 0, "Schema should have fields");
}

#[tokio::test]
async fn test_data_table_schema() {
    let data_dir = random_memory_dir();
    let mut bundle = BundleBuilder::create(data_dir.url().as_str(), None)
        .await
        .unwrap();

    // Attach data
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Get dataframe to populate cache
    let df = bundle.dataframe().await.unwrap();
    let df_schema = df.schema();

    // Query via data table
    let result = bundle.bundle.ctx().sql("SELECT * FROM bundle").await.unwrap();
    let result_schema = result.schema();

    // Schemas should match
    assert_eq!(
        df_schema.fields().len(),
        result_schema.fields().len(),
        "Data table schema should match dataframe schema"
    );

    // Check field names match
    for (df_field, result_field) in df_schema.fields().iter().zip(result_schema.fields().iter()) {
        assert_eq!(
            df_field.name(),
            result_field.name(),
            "Field names should match"
        );
    }
}
