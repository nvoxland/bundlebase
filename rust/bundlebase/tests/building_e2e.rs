use bundlebase;
use bundlebase::bundle::{AnyOperation, BundleFacade, InitCommit, INIT_FILENAME};
use bundlebase::io::ObjectStoreFile;
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use bundlebase::Bundle;
use url::Url;

mod common;

#[tokio::test]
async fn test_extend_to_different_directory() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_dir();
    let temp2 = random_memory_dir();

    // Create and commit first bundle
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.to_string()).await?;
    assert_eq!(None, c1.bundle.from());
    assert_eq!(temp1.url(), c1.url());
    c1.attach(test_datafile("customers-0-100.csv")).await?;
    c1.commit("Initial commit").await?;

    let init_commit = temp1.subdir("_manifest")?.file(INIT_FILENAME)?;
    let init_commit: Option<InitCommit> = init_commit.read_yaml().await?;
    let init_commit = init_commit.expect("Failed to read init commit");
    assert_eq!(None, init_commit.from);
    assert_eq!(None, c1.bundle.from());

    // Open first bundle and extend to new directory
    let opened1 = Bundle::open(&temp1.to_string()).await?;
    assert_eq!(opened1.operations().len(), 2);
    assert_eq!(None, opened1.from());
    assert_eq!(temp1.url(), opened1.url());

    let mut c2 = opened1.extend(&temp2.to_string())?;
    assert_eq!(Some(temp1.url()), c2.bundle.from());
    assert_eq!(temp2.url(), c2.url());

    // Add operation to extended bundle
    c2.remove_column("country").await?;
    c2.commit("Remove country column").await?;
    assert_eq!(Some(temp1.url()), c2.bundle.from());

    let init_commit = temp2.subdir("_manifest")?.file(INIT_FILENAME)?;
    let init_commit: Option<InitCommit> = init_commit.read_yaml().await?;
    let init_commit = init_commit.expect("Failed to read init commit");
    assert_eq!(Some(temp1.url().clone()), init_commit.from);

    // Open the extended bundle
    let reopened = Bundle::open(&temp2.to_string()).await?;
    assert_eq!(Some(temp1.url()), c2.bundle.from());
    assert_eq!(reopened.url(), c2.url());

    // Verify the schema doesn't have country
    assert!(!common::has_column(&reopened.schema().await?, "country"));
    // The number of operations should include both from base and new
    // Since we're extending from path1, it should have attach + remove
    assert!(reopened.operations().len() >= 1); // At least the remove_column

    Ok(())
}

#[tokio::test]
async fn test_simple_extend_chain() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_url();
    let temp2 = random_memory_url();

    // Create base bundle
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.to_string()).await?;
    c1.attach(test_datafile("customers-0-100.csv")).await?;
    c1.commit("Base commit").await?;

    // Extend and commit
    let base1 = Bundle::open(&temp1.to_string()).await?;
    assert_eq!(1, base1.history().len());
    let mut c2 = base1.extend(&temp2.to_string())?;
    c2.remove_column("country").await?;
    c2.commit("Extended commit").await?;

    // Reopen extended bundle and verify history
    let reopened = Bundle::open(&temp2.to_string()).await?;
    let history = reopened.history();

    assert_eq!(
        history.len(),
        2,
        "Expected 2 commits in history, got {}",
        history.len()
    );
    assert_eq!(history[0].message, "Base commit");
    assert_eq!(history[1].message, "Extended commit");

    Ok(())
}

#[tokio::test]
async fn test_lazy_history_traversal() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_url();
    let temp2 = random_memory_url();
    let temp3 = random_memory_url();

    // Create 3-level bundle chain
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.to_string()).await?;
    c1.attach(test_datafile("customers-0-100.csv")).await?;
    c1.commit("Base commit").await?;

    let base1 = Bundle::open(&temp1.to_string()).await?;
    let mut c2 = base1.extend(&temp2.to_string())?;
    c2.remove_column("country").await?;
    c2.commit("Second commit").await?;

    let base2 = Bundle::open(&temp2.to_string()).await?;
    let mut c3 = base2.extend(&temp3.to_string())?;
    c3.remove_column("phone").await?;
    c3.commit("Third commit").await?;

    let final_bundle = Bundle::open(&temp3.to_string()).await?;

    let history = final_bundle.history();

    // Verify we can get the full history by traversing the Arc chain
    assert_eq!(history.len(), 3);

    // Verify the messages match the commits we made
    assert_eq!(history[0].message, "Base commit");
    assert_eq!(history[1].message, "Second commit");
    assert_eq!(history[2].message, "Third commit");

    Ok(())
}

#[tokio::test]
async fn test_operations_stored_in_state() -> Result<(), BundlebaseError> {
    let temp = random_memory_url();

    let mut bundle = bundlebase::BundleBuilder::create(&temp.to_string()).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"))
        .await?;
    bundle.remove_column("country").await?;

    assert_eq!(bundle.bundle.operations().len(), 3);
    assert_eq!(bundle.bundle.operations().len(), 3);

    bundle.commit("Test commit").await?;

    // After commit, reopen the bundle
    let reopened = Bundle::open(&temp.to_string()).await?;

    // Operations should now be in state
    assert_eq!(reopened.operations().len(), 3);

    Ok(())
}

#[tokio::test]
async fn test_extend_with_relative_paths() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_dir();
    let temp2 = random_memory_dir();

    // Create Bundle A with attachment using RELATIVE path
    let mut bundle_a = bundlebase::BundleBuilder::create(&temp1.to_string()).await?;

    // Copy test data to bundle's directory with a local name
    let source_file = test_datafile("customers-0-100.csv");
    let local_file = temp1.file("local_data.csv")?;

    // Read source data and write to local location
    let source_obj = ObjectStoreFile::from_url(&Url::parse(source_file)?)?;
    let data = source_obj.read_bytes().await?.expect("Failed to read source file");
    local_file.write(data).await?;

    // Attach using relative path (no scheme separator)
    bundle_a.attach("local_data.csv").await?;
    bundle_a.commit("Bundle A with relative path").await?;

    // Extend to Bundle B in different location
    let bundle_a_reopened = Bundle::open(&temp1.to_string()).await?;
    let mut bundle_b = bundle_a_reopened.extend(&temp2.to_string())?;
    bundle_b.remove_column("country").await?;
    bundle_b.commit("Bundle B extends A").await?;

    // Reopen Bundle B - this should work without file-not-found errors
    let bundle_b_reopened = Bundle::open(&temp2.to_string()).await?;

    // Verify data is accessible
    let df = bundle_b_reopened.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(batches[0].num_rows() > 0, "Should have rows from the attached file");

    // Verify country column was removed
    assert!(!common::has_column(&bundle_b_reopened.schema().await?, "country"));

    // Verify the operation was normalized to absolute path
    let operations = bundle_b_reopened.operations();
    let attach_op = operations
        .iter()
        .find_map(|op| match op {
            AnyOperation::AttachBlock(attach) => Some(attach),
            _ => None,
        })
        .expect("Should have AttachBlock operation");

    // Path should now be absolute (contains scheme)
    assert!(
        attach_op.source.contains(':'),
        "Path should be absolute after normalization: {}",
        attach_op.source
    );

    // Path should point to Bundle A's location
    assert!(
        attach_op.source.starts_with("memory:///"),
        "Path should be a memory URL: {}",
        attach_op.source
    );

    Ok(())
}

#[tokio::test]
async fn test_all_relative_paths_normalized() -> Result<(), BundlebaseError> {
    let temp = random_memory_dir();

    // Create bundle with relative path
    let mut bundle = bundlebase::BundleBuilder::create(&temp.to_string()).await?;

    // Copy data to bundle directory
    let source_file = test_datafile("customers-0-100.csv");
    let local_file = temp.file("my_data.csv")?;

    let source_obj = ObjectStoreFile::from_url(&Url::parse(source_file)?)?;
    let data = source_obj.read_bytes().await?.expect("Failed to read source file");
    local_file.write(data).await?;

    bundle.attach("my_data.csv").await?;
    bundle.commit("Relative path").await?;

    // Reopen - all relative paths are normalized to absolute URLs
    let reopened = Bundle::open(&temp.to_string()).await?;
    let df = reopened.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(batches[0].num_rows() > 0, "Should have rows from the attached file");

    // Verify the operation path was normalized to absolute
    let attach_op = reopened.operations()
        .iter()
        .find_map(|op| match op {
            AnyOperation::AttachBlock(attach) => Some(attach),
            _ => None,
        })
        .expect("Should have AttachBlock operation");

    // Path should be normalized to absolute (contains scheme)
    assert!(
        attach_op.source.contains(':'),
        "Path should be normalized to absolute: {}",
        attach_op.source
    );

    // YAML should also have absolute path (normalization happens at commit time)
    let (_, commit, _) = common::latest_commit(&temp).await?.expect("Should have a commit");
    let yaml_op = commit.changes[0].operations
        .iter()
        .find_map(|op| match op {
            AnyOperation::AttachBlock(attach) => Some(attach),
            _ => None,
        })
        .expect("Should have AttachBlock operation in YAML");

    assert!(
        yaml_op.source.contains(':'),
        "YAML should have absolute path: {}",
        yaml_op.source
    );

    Ok(())
}

#[tokio::test]
async fn test_index_path_normalization() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_dir();
    let temp2 = random_memory_dir();

    // Create Bundle A with indexed data
    let mut bundle_a = bundlebase::BundleBuilder::create(&temp1.to_string()).await?;

    // Copy test data to bundle's directory
    let source_file = test_datafile("userdata.parquet");
    let local_file = temp1.file("data.parquet")?;
    let source_obj = ObjectStoreFile::from_url(&Url::parse(source_file)?)?;
    let data = source_obj.read_bytes().await?.expect("Failed to read source file");
    local_file.write(data).await?;

    // Attach and create index
    bundle_a.attach("data.parquet").await?;
    bundle_a.index("id").await?;
    bundle_a.commit("Bundle A with index").await?;

    // Extend to Bundle B in different location
    let bundle_a_reopened = Bundle::open(&temp1.to_string()).await?;
    let mut bundle_b = bundle_a_reopened.extend(&temp2.to_string())?;
    bundle_b.remove_column("country").await?;
    bundle_b.commit("Bundle B extends A").await?;

    // Reopen Bundle B - this should work without file-not-found errors for index files
    let bundle_b_reopened = Bundle::open(&temp2.to_string()).await?;

    // Verify data is accessible
    let df = bundle_b_reopened.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(batches[0].num_rows() > 0, "Should have rows from the attached file");

    // Verify the IndexBlocksOp has absolute path
    let operations = bundle_b_reopened.operations();
    let index_blocks_op = operations
        .iter()
        .find_map(|op| match op {
            AnyOperation::IndexBlocks(index) => Some(index),
            _ => None,
        })
        .expect("Should have IndexBlocks operation");

    assert!(
        index_blocks_op.path.contains(':'),
        "Index path should be absolute: {}",
        index_blocks_op.path
    );

    // Verify AttachBlockOp layout field is also absolute (if present)
    let attach_op = operations
        .iter()
        .find_map(|op| match op {
            AnyOperation::AttachBlock(attach) => Some(attach),
            _ => None,
        })
        .expect("Should have AttachBlock operation");

    if let Some(ref layout) = attach_op.layout {
        assert!(
            layout.contains(':'),
            "Layout path should be absolute: {}",
            layout
        );
    }

    Ok(())
}
