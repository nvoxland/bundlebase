use bundlebase::bundle::{BundleBuilder, BundleFacade};
use bundlebase::test_utils::test_datafile;
use bundlebase_command::{BundleBuilderExt, BundleFacadeCommandExt};
use bundlebase_common::BundlebaseError;

mod common;

fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| { bundlebase_catalog::init(); });
}

#[tokio::test]
async fn test_schema_after_attach() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    let schema = bundle.schema().await.unwrap();
    assert!(
        !schema.fields().is_empty(),
        "After attach, schema should have fields"
    );
    assert_eq!(schema.fields().len(), 13, "userdata.parquet has 13 columns");

    // Verify specific column names exist
    let field_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    assert!(field_names.contains(&"id".to_string()));
    assert!(field_names.contains(&"first_name".to_string()));
    assert!(field_names.contains(&"email".to_string()));
}

#[tokio::test]
async fn test_schema_after_drop_column() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    let schema_before = bundle.schema().await.unwrap();
    assert_eq!(schema_before.fields().len(), 13);

    bundle.drop_column("title").await.unwrap();
    let schema_after = bundle.schema().await.unwrap();
    assert_eq!(schema_after.fields().len(), 12);

    // Verify 'title' column is gone
    let field_names: Vec<String> = schema_after
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert!(!field_names.contains(&"title".to_string()));
}

#[tokio::test]
async fn test_set_and_get_name() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    assert_eq!(bundle.name(), None, "Empty bundle should have no name");

    bundle.set_name("My Bundle").await.unwrap();
    assert_eq!(bundle.name(), Some("My Bundle".to_string()));
}

#[tokio::test]
async fn test_set_and_get_description() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    assert_eq!(bundle.description(), None);

    bundle
        .set_description("This is a test bundle")
        .await
        .unwrap();
    assert_eq!(
        bundle.description(),
        Some("This is a test bundle".to_string())
    );
}

#[tokio::test]
async fn test_name_tracked_as_operation() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    bundle.set_name("Named Bundle").await.unwrap();

    // Verify the name was actually set
    assert_eq!(bundle.name(), Some("Named Bundle".to_string()));
    // set_name should show up in status as a change
    assert!(!bundle.status().is_empty(), "Status should reflect name change");
}

#[tokio::test]
async fn test_operations_list() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    assert!(bundle.status().is_empty(), "Empty bundle should have no status changes");

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    assert!(!bundle.status().is_empty(), "Status should have changes after attach");

    let ops_after_attach = bundle.operations().len();
    bundle.drop_column("title").await.unwrap();
    assert!(bundle.operations().len() > ops_after_attach, "Operations should grow after drop_column");
}

#[tokio::test]
async fn test_version() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();

    assert_eq!(bundle.version(), "empty");

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    assert_eq!(bundle.version(), "UNCOMMITTED");
}

#[tokio::test]
async fn test_multiple_operations_pipeline() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.drop_column("title").await.unwrap();
    bundle
        .rename_column("first_name", "given_name")
        .await
        .unwrap();

    // Verify all 3 operations are tracked
    assert!(bundle.operations().len() >= 3, "Should have at least 3 operations");
    // Verify the rename took effect
    let schema = bundle.schema().await.unwrap();
    let field_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
    assert!(field_names.contains(&"given_name".to_string()));
    assert!(!field_names.contains(&"first_name".to_string()));
    assert!(!field_names.contains(&"title".to_string()));
}

#[tokio::test]
async fn test_version_uncommitted_with_changes() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    assert_eq!(bundle.version(), "empty");

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    assert_eq!(bundle.version(), "UNCOMMITTED");
}

#[tokio::test]
async fn test_version_temp_with_temporary_connector_only() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();

    bundle
        .import_temp_connector("test.source", "docker::test-image:latest", "*/*")
        .await
        .unwrap();

    assert_eq!(bundle.version(), "TEMP");
}

#[tokio::test]
async fn test_version_uncommitted_temp_with_changes_and_temporary_connector() {
    init();
    let bundle = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    bundle
        .import_temp_connector("test.source", "docker::test-image:latest", "*/*")
        .await
        .unwrap();

    assert_eq!(bundle.version(), "UNCOMMITTED+TEMP");
}

#[tokio::test]
async fn test_version_uncommitted_temp_via_facade() {
    init();
    let builder = BundleBuilder::create("memory:///test_bundle", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    builder
        .import_temp_connector("test.source", "docker::test-image:latest", "*/*")
        .await
        .unwrap();

    assert_eq!(builder.version(), "UNCOMMITTED+TEMP");
}

#[tokio::test]
async fn test_commit_blocked_by_temp_function_in_filter() -> Result<(), BundlebaseError> {
    init();
    use bundlebase::bundle::function_entry::{FunctionEntry, FunctionKind, parse_function_name};
    use bundlebase_udf::UdfRuntime;
    use bundlebase_common::platform::Platform;
    use bundlebase_udf::bridge::ipc_bridge::new_subprocess_cache;
    use bundlebase_common::object_id::ObjectId;
    use arrow::datatypes::DataType;
    use datafusion::logical_expr::ScalarUDF;

    let builder = BundleBuilder::create("memory:///test_temp_guard", None)
        .await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Manually add a temp function entry and register the UDF
    let entry = FunctionEntry {
        id: ObjectId::generate(),
        name: parse_function_name("test.double_val").unwrap(),
        input_types: vec![DataType::Int32],
        return_type: DataType::Int32,
        from: UdfRuntime::parse_from("ipc::fake_binary").unwrap(),
        platform: Platform::any(),
        temporary: true,
        kind: FunctionKind::Scalar,
    };
    let func = bundlebase_udf::bridge::scalar::ScalarFunction::new_composite(
        vec![entry.clone()],
        new_subprocess_cache(),
    )
    .unwrap();
    builder.bundle().ctx().register_udf(ScalarUDF::from(func));
    builder.bundle().function_registry().write().add(entry);

    // Verify temp function is registered
    let temp_names = builder.bundle().function_registry().read().temporary_only_names();
    assert!(
        temp_names.contains(&"test.double_val".to_string()),
        "Expected 'test.double_val' in temp_names: {:?}",
        temp_names
    );

    // Apply a filter that uses the temp function
    builder
        .filter("SELECT * FROM bundle WHERE test.double_val(id) > 10", vec![])
        .await?;

    // Verify status has the filter operation
    let status = builder.status();
    assert!(!status.is_empty(), "Status should have changes after filter");

    // Commit should fail
    let result = builder.commit("should fail").await;
    assert!(result.is_err(), "Commit should fail when filter uses temp function");
    let err = result.err().unwrap();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("temporary function"),
        "Error should mention temp function: {}",
        err_msg
    );

    Ok(())
}

#[tokio::test]
async fn test_commit_succeeds_without_temp_function_in_filter() {
    init();
    let builder = BundleBuilder::create("memory:///test_no_temp", None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();

    // Filter without temp function
    builder
        .filter("SELECT * FROM bundle WHERE id > 10", vec![])
        .await
        .unwrap();

    // Commit should succeed
    let result = builder.commit("should succeed").await;
    assert!(result.is_ok(), "Commit should succeed: {:?}", result.err());
}

// --- RESET tests ---

#[tokio::test]
async fn test_reset_before_first_commit_leaves_bundle_usable() {
    // Bug: RESET on a never-committed bundle dropped the BASE_PACK from the
    // in-memory state, causing subsequent ATTACH operations to fail.
    init();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    let bundle = BundleBuilder::create(path, None).await.unwrap();

    // Attach then reset (no commit yet)
    bundle.attach(test_datafile("userdata.parquet"), None).await.unwrap();
    bundle.reset().await.unwrap();

    // Should be able to attach again after reset
    bundle.attach(test_datafile("userdata.parquet"), None).await
        .expect("ATTACH after RESET (before first commit) should succeed");

    // Should be able to commit
    bundle.commit("initial").await
        .expect("COMMIT after RESET (before first commit) should succeed");

    // Reopen and verify the data survived
    let reopened = bundlebase::bundle::Bundle::open(path, None).await.unwrap();
    assert_eq!(1000, reopened.num_rows().await.unwrap());
}

#[tokio::test]
async fn test_reset_after_commit_preserves_init_file_and_data() {
    // RESET after a commit should reload from the committed state without
    // touching the INIT file or any committed manifest files.
    init();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    let bundle = BundleBuilder::create(path, None).await.unwrap();
    bundle.attach(test_datafile("userdata.parquet"), None).await.unwrap();
    bundle.commit("initial").await.unwrap();

    // Add more (uncommitted) data then reset
    bundle.attach(test_datafile("userdata.parquet"), None).await.unwrap();
    bundle.reset().await.unwrap();

    // Should be back to the committed state (1000 rows, not 2000)
    assert_eq!(1000, bundle.num_rows().await.unwrap(),
        "After RESET, row count should match last commit");

    // The INIT file must still be on disk (META_DIR = "_bundlebase")
    let meta_path = std::path::Path::new(path)
        .join("_bundlebase")
        .join("00000000000000000.yaml");
    assert!(meta_path.exists(), "INIT file must still exist after RESET");

    // Bundle must still be reopenable
    let reopened = bundlebase::bundle::Bundle::open(path, None).await
        .expect("Bundle must be reopenable after RESET");
    assert_eq!(1000, reopened.num_rows().await.unwrap());
}
