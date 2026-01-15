use arrow::array::record_batch;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::functions::{FunctionSignature, StaticImpl};
use bundlebase::test_utils::random_memory_url;
use bundlebase::BundlebaseError;
use std::sync::Arc;

mod common;

#[tokio::test]
#[ignore] // TODO: Manifest verification with memory URLs needs proper storage access
async fn function_datasource() -> Result<(), BundlebaseError> {
    let base_url = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(base_url.as_str(), None).await?;

    // Define a function
    bundle
        .create_function(FunctionSignature::new(
            "test_func",
            SchemaRef::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("value", DataType::Utf8, true),
            ])),
        ))
        .await?;
    bundle
        .set_impl(
            "test_func",
            Arc::new(StaticImpl::new(
                vec![record_batch!(
                    ("id", Int64, [1, 2, 3]),
                    ("value", Utf8, ["a", "b", "c"])
                )?],
                "func-version".to_string(),
            )),
        )
        .await?;
    bundle.attach("function://test_func", None).await?;

    // Save bundle
    bundle.commit("Commit changes").await?;

    // Verify the bundle is functional after commit
    assert_eq!(
        3,
        bundle.num_rows().await?,
        "Should have 3 rows from function"
    );

    Ok(())
}

#[tokio::test]
async fn test_function_with_static_impl_basic() -> Result<(), BundlebaseError> {
    // Test 1: Define a function and attach it with StaticImpl
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Create schema for our function
    let schema = SchemaRef::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
    ]));

    // Define the function
    let sig = FunctionSignature::new("users", schema.clone());
    bundle.create_function(sig).await?;

    // Create test data
    let test_data = vec![record_batch!(
        ("id", Int64, [1_i64, 2_i64, 3_i64]),
        ("name", Utf8, ["Alice", "Bob", "Charlie"])
    )?];

    // Set implementation
    let impl_arc = Arc::new(StaticImpl::new(test_data, "v1".to_string()));
    bundle.set_impl("users", impl_arc).await?;

    // Attach the function
    bundle.attach("function://users", None).await?;

    // Query the data
    let df = bundle.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1, "Should have one record batch");
    assert_eq!(result[0].num_rows(), 3, "Should have 3 rows");

    Ok(())
}
#[tokio::test]
async fn test_function_with_multiple_pages() -> Result<(), BundlebaseError> {
    // Test 2: Function that returns data across multiple pages
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let schema = SchemaRef::new(Schema::new(vec![Field::new(
        "page_num",
        DataType::Int32,
        false,
    )]));

    let sig = FunctionSignature::new("paginated", schema.clone());
    bundle.create_function(sig).await?;

    // Create data for 2 pages
    let page_0 = record_batch!(("page_num", Int32, [0_i32, 0_i32]))?;
    let page_1 = record_batch!(("page_num", Int32, [1_i32, 1_i32]))?;

    let impl_arc = Arc::new(StaticImpl::new(vec![page_0, page_1], "v1".to_string()));
    bundle.set_impl("paginated", impl_arc).await?;
    bundle.attach("function://paginated", None).await?;

    let df = bundle.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    // Both pages should be combined into the result
    let total_rows: usize = result.iter().map(|batch| batch.num_rows()).sum();
    assert_eq!(total_rows, 4, "Should have 4 total rows from 2 pages");

    Ok(())
}
#[tokio::test]
async fn test_function_in_pipeline_with_transformations() -> Result<(), BundlebaseError> {
    // Test 3: Function data in a transformation pipeline
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let schema = SchemaRef::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Int32, false),
    ]));

    let sig = FunctionSignature::new("scores", schema.clone());
    bundle.create_function(sig).await?;

    let test_data = vec![record_batch!(
        ("id", Int64, [1_i64, 2_i64, 3_i64]),
        ("name", Utf8, ["Alice", "Bob", "Charlie"]),
        ("score", Int32, [90_i32, 85_i32, 95_i32])
    )?];

    let impl_arc = Arc::new(StaticImpl::new(test_data, "v1".to_string()));
    bundle.set_impl("scores", impl_arc).await?;
    bundle.attach("function://scores", None).await?;

    // Apply transformations
    bundle.drop_column("score").await?;
    bundle.rename_column("name", "full_name").await?;

    // Verify schema by checking columns
    let schema = bundle.schema().await?;
    assert!(
        schema.column_with_name("id").is_some(),
        "Should have id column"
    );
    assert!(
        schema.column_with_name("full_name").is_some(),
        "Should have renamed column"
    );
    assert!(
        schema.column_with_name("score").is_none(),
        "score should be removed"
    );

    // Query the transformed data
    let df = bundle.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;

    assert_eq!(result.len(), 1, "Should have one record batch");
    assert_eq!(result[0].num_rows(), 3, "Should have 3 rows");
    assert_eq!(
        result[0].num_columns(),
        2,
        "Should have 2 columns (id, full_name)"
    );

    Ok(())
}
#[tokio::test]
async fn test_multiple_functions_in_bundle() -> Result<(), BundlebaseError> {
    // Test 4: Multiple different functions defined and used
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Define first function
    let schema1 = SchemaRef::new(Schema::new(vec![
        Field::new("user_id", DataType::Int64, false),
        Field::new("username", DataType::Utf8, true),
    ]));
    let sig1 = FunctionSignature::new("users", schema1.clone());
    bundle.create_function(sig1).await?;

    // Define second function
    let schema2 = SchemaRef::new(Schema::new(vec![
        Field::new("product_id", DataType::Int64, false),
        Field::new("product_name", DataType::Utf8, true),
    ]));
    let sig2 = FunctionSignature::new("products", schema2.clone());
    bundle.create_function(sig2).await?;

    // Set implementations
    let user_data = vec![record_batch!(
        ("user_id", Int64, [1_i64, 2_i64]),
        ("username", Utf8, ["alice", "bob"])
    )?];
    let impl1 = Arc::new(StaticImpl::new(user_data, "v1".to_string()));
    bundle.set_impl("users", impl1).await?;

    let product_data = vec![record_batch!(
        ("product_id", Int64, [100_i64, 101_i64, 102_i64]),
        ("product_name", Utf8, ["Book", "Pen", "Notebook"])
    )?];
    let impl2 = Arc::new(StaticImpl::new(product_data, "v1".to_string()));
    bundle.set_impl("products", impl2).await?;

    // Attach users
    bundle.attach("function://users", None).await?;
    let df = bundle.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;
    assert_eq!(result[0].num_rows(), 2, "Users function should have 2 rows");

    Ok(())
}
#[tokio::test]
async fn test_function_with_metadata() -> Result<(), BundlebaseError> {
    // Test 5: Functions with name and description metadata
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let schema = SchemaRef::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("value", DataType::Utf8, true),
    ]));

    let sig = FunctionSignature::new("test_func", schema.clone());
    bundle.create_function(sig).await?;

    let test_data = vec![record_batch!(
        ("id", Int64, [1_i64]),
        ("value", Utf8, ["test"])
    )?];

    let impl_arc = Arc::new(StaticImpl::new(test_data, "v1".to_string()));
    bundle.set_impl("test_func", impl_arc).await?;
    bundle.attach("function://test_func", None).await?;

    // Set name and description
    bundle.set_name("FunctionBundleTest").await?;
    bundle.set_description("Test bundle with function").await?;

    // Verify metadata
    assert_eq!(bundle.name().as_deref(), Some("FunctionBundleTest"));
    assert_eq!(
        bundle.description().as_deref(),
        Some("Test bundle with function")
    );

    // Verify data still works
    let df = bundle.dataframe().await?;
    let result = df.as_ref().clone().collect().await?;
    assert_eq!(result[0].num_rows(), 1, "Should have 1 row");

    Ok(())
}
#[tokio::test]
async fn test_function_error_no_implementation() -> Result<(), BundlebaseError> {
    // Test 6: Error handling when function implementation is not set
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let schema = SchemaRef::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));

    let sig = FunctionSignature::new("missing_impl", schema);
    bundle.create_function(sig).await?;

    // Try to attach without setting implementation - should fail
    let result = bundle.attach("function://missing_impl", None).await;
    assert!(
        result.is_err(),
        "Should fail when implementation is not set"
    );

    Ok(())
}
#[tokio::test]
async fn test_function_error_unknown_function() -> Result<(), BundlebaseError> {
    // Test 7: Error handling when trying to attach unknown function
    let mut bundle = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Try to attach a function that was never defined
    let result = bundle.attach("function://undefined_func", None).await;
    assert!(result.is_err(), "Should fail when function is not defined");

    Ok(())
}
#[tokio::test]
async fn test_multiple_function_definitions() -> Result<(), BundlebaseError> {
    // Test 8: Define multiple functions and verify they're independent
    // Create first bundle for func1
    let mut bundle1 = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Define func1
    let schema1 = SchemaRef::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]));
    let sig1 = FunctionSignature::new("func1", schema1);
    bundle1.create_function(sig1).await?;

    // Set implementation for func1
    let data1 = vec![record_batch!(("x", Int32, [1_i32, 2_i32, 3_i32]))?];
    let impl1 = Arc::new(StaticImpl::new(data1, "v1".to_string()));
    bundle1.set_impl("func1", impl1).await?;

    // Attach and query func1
    bundle1.attach("function://func1", None).await?;
    let df1 = bundle1.dataframe().await?;
    let result1 = df1.as_ref().clone().collect().await?;
    assert_eq!(result1[0].num_rows(), 3, "func1 should have 3 rows");

    // Create second bundle for func2
    let mut bundle2 = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    // Define func2
    let schema2 = SchemaRef::new(Schema::new(vec![Field::new("y", DataType::Utf8, true)]));
    let sig2 = FunctionSignature::new("func2", schema2);
    bundle2.create_function(sig2).await?;

    // Set implementation for func2
    let data2 = vec![record_batch!(("y", Utf8, ["a", "b"]))?];
    let impl2 = Arc::new(StaticImpl::new(data2, "v1".to_string()));
    bundle2.set_impl("func2", impl2).await?;

    // Attach and query func2
    bundle2.attach("function://func2", None).await?;
    let df2 = bundle2.dataframe().await?;
    let result2 = df2.as_ref().clone().collect().await?;
    assert_eq!(result2[0].num_rows(), 2, "func2 should have 2 rows");

    Ok(())
}
