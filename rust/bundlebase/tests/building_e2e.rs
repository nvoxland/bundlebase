use bundlebase;
use bundlebase::bundle::{AnyOperation, BundleFacade, InitCommit, INIT_FILENAME, META_DIR};
use bundlebase::io::{read_yaml, readable_file_from_url};
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::Bundle;
use bundlebase::BundleConfig;
use bundlebase::BundlebaseError;
use url::Url;

mod common;

#[tokio::test]
async fn test_extend_to_different_directory() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_dir();
    let temp2 = random_memory_dir();

    // Create and commit first bundle
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.url().to_string(), None).await?;
    assert_eq!(None, c1.bundle().from());
    assert_eq!(*temp1.url(), c1.url());
    c1.attach(test_datafile("customers-0-100.csv"), None).await?;
    c1.commit("Initial commit").await?;

    let init_commit = temp1.subdir(META_DIR)?.file(INIT_FILENAME)?;
    let init_commit: Option<InitCommit> = read_yaml(init_commit.as_ref()).await?;
    let init_commit = init_commit.expect("Failed to read init commit");
    assert_eq!(None, init_commit.from);
    assert_eq!(None, c1.bundle().from());

    // Open first bundle and extend to new directory
    let opened1 = Bundle::open(&temp1.url().to_string(), None).await?;
    assert_eq!(opened1.operations().len(), 1);
    assert_eq!(None, opened1.from());
    assert_eq!(*temp1.url(), opened1.url());

    let mut c2 = opened1.extend(Some(&temp2.url().to_string()))?;
    assert_eq!(Some(temp1.url()), c2.bundle().from().as_ref());
    assert_eq!(*temp2.url(), c2.url());

    // Add operation to extended bundle
    c2.drop_column("country").await?;
    c2.commit("Remove country column").await?;
    assert_eq!(Some(temp1.url()), c2.bundle().from().as_ref());

    let init_commit = temp2.subdir(META_DIR)?.file(INIT_FILENAME)?;
    let init_commit: Option<InitCommit> = read_yaml(init_commit.as_ref()).await?;
    let init_commit = init_commit.expect("Failed to read init commit");
    assert_eq!(Some(temp1.url().clone()), init_commit.from);

    // Open the extended bundle
    let reopened = Bundle::open(&temp2.url().to_string(), None).await?;
    assert_eq!(Some(temp1.url()), c2.bundle().from().as_ref());
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
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.to_string(), None).await?;
    c1.attach(test_datafile("customers-0-100.csv"), None).await?;
    c1.commit("Base commit").await?;

    // Extend and commit
    let base1 = Bundle::open(&temp1.to_string(), None).await?;
    assert_eq!(1, base1.history().len());
    let mut c2 = base1.extend(Some(&temp2.to_string()))?;
    c2.drop_column("country").await?;
    c2.commit("Extended commit").await?;

    // Reopen extended bundle and verify history
    let reopened = Bundle::open(&temp2.to_string(), None).await?;
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
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.to_string(), None).await?;
    c1.attach(test_datafile("customers-0-100.csv"), None).await?;
    c1.commit("Base commit").await?;

    let base1 = Bundle::open(&temp1.to_string(), None).await?;
    let mut c2 = base1.extend(Some(&temp2.to_string()))?;
    c2.drop_column("country").await?;
    c2.commit("Second commit").await?;

    let base2 = Bundle::open(&temp2.to_string(), None).await?;
    let mut c3 = base2.extend(Some(&temp3.to_string()))?;
    c3.drop_column("phone").await?;
    c3.commit("Third commit").await?;

    let final_bundle = Bundle::open(&temp3.to_string(), None).await?;

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

    let mut bundle = bundlebase::BundleBuilder::create(&temp.to_string(), None).await?;
    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;
    bundle.drop_column("country").await?;

    assert_eq!(bundle.bundle().operations().len(), 2);
    assert_eq!(bundle.bundle().operations().len(), 2);

    bundle.commit("Test commit").await?;

    // After commit, reopen the bundle
    let reopened = Bundle::open(&temp.to_string(), None).await?;

    // Operations should now be in state
    assert_eq!(reopened.operations().len(), 2);

    Ok(())
}

#[tokio::test]
async fn test_extend_with_relative_paths() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_dir();
    let temp2 = random_memory_dir();

    // Create Bundle A with attachment using RELATIVE path
    let mut bundle_a = bundlebase::BundleBuilder::create(&temp1.url().to_string(), None).await?;

    // Copy test data to bundle's directory with a local name
    let source_file = test_datafile("customers-0-100.csv");
    let local_file = temp1.writable_file("local_data.csv")?;

    // Read source data and write to local location
    let source_obj =
        readable_file_from_url(&Url::parse(source_file)?, BundleConfig::default().into())?;
    let data: bytes::Bytes = source_obj
        .read_bytes()
        .await?
        .expect("Failed to read source file");
    local_file.write(data).await?;

    // Attach using relative path (no scheme separator)
    bundle_a.attach("local_data.csv", None).await?;
    bundle_a.commit("Bundle A with relative path").await?;

    // Extend to Bundle B in different location
    let bundle_a_reopened = Bundle::open(&temp1.url().to_string(), None).await?;
    let mut bundle_b = bundle_a_reopened.extend(Some(&temp2.url().to_string()))?;
    bundle_b.drop_column("country").await?;
    bundle_b.commit("Bundle B extends A").await?;

    // Reopen Bundle B - this should work without file-not-found errors
    let bundle_b_reopened = Bundle::open(&temp2.url().to_string(), None).await?;

    // Verify data is accessible
    let df = bundle_b_reopened.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(
        batches[0].num_rows() > 0,
        "Should have rows from the attached file"
    );

    // Verify country column was removed
    assert!(!common::has_column(
        &bundle_b_reopened.schema().await?,
        "country"
    ));

    let operations = bundle_b_reopened.operations();
    let attach_op = operations
        .iter()
        .find_map(|op| match op {
            AnyOperation::AttachBlock(attach) => Some(attach),
            _ => None,
        })
        .expect("Should have AttachBlock operation");

    assert!(!attach_op.location.contains(':'));

    // Path should point to Bundle A's location
    assert_eq!("local_data.csv", attach_op.location);

    Ok(())
}

#[tokio::test]
async fn test_extend_inherits_same_id() -> Result<(), BundlebaseError> {
    let temp1 = random_memory_dir();
    let temp2 = random_memory_dir();
    let temp3 = random_memory_dir();

    // Create base bundle
    let mut c1 = bundlebase::BundleBuilder::create(&temp1.url().to_string(), None).await?;
    c1.attach(test_datafile("customers-0-100.csv"), None).await?;
    c1.commit("Initial commit").await?;

    // Get the ID from the base bundle's InitCommit
    let init_file1 = temp1.subdir(META_DIR)?.file(INIT_FILENAME)?;
    let init_commit1: InitCommit = read_yaml(init_file1.as_ref())
        .await?
        .expect("Should have init commit");
    let base_id = init_commit1.id.expect("Base bundle should have id");
    assert!(init_commit1.from.is_none(), "Base bundle should not have 'from'");

    // Extend to second bundle
    let base1 = Bundle::open(&temp1.url().to_string(), None).await?;
    assert_eq!(base_id, base1.id(), "Opened bundle should have same ID as InitCommit");

    let mut c2 = base1.extend(Some(&temp2.url().to_string()))?;
    c2.drop_column("country").await?;
    c2.commit("Second commit").await?;

    // Verify extended bundle's InitCommit has only 'from', not 'id'
    let init_file2 = temp2.subdir(META_DIR)?.file(INIT_FILENAME)?;
    let init_commit2: InitCommit = read_yaml(init_file2.as_ref())
        .await?
        .expect("Should have init commit");
    assert!(init_commit2.id.is_none(), "Extended bundle should NOT have 'id' in InitCommit");
    assert_eq!(
        Some(temp1.url().clone()),
        init_commit2.from,
        "Extended bundle should have 'from' pointing to parent"
    );

    // Verify the opened extended bundle has the SAME id as the base bundle
    let base2 = Bundle::open(&temp2.url().to_string(), None).await?;
    assert_eq!(
        base_id,
        base2.id(),
        "Extended bundle should inherit the same ID as base bundle"
    );

    // Extend again to third bundle and verify ID is still the same
    let mut c3 = base2.extend(Some(&temp3.url().to_string()))?;
    c3.drop_column("phone").await?;
    c3.commit("Third commit").await?;

    let init_file3 = temp3.subdir(META_DIR)?.file(INIT_FILENAME)?;
    let init_commit3: InitCommit = read_yaml(init_file3.as_ref())
        .await?
        .expect("Should have init commit");
    assert!(init_commit3.id.is_none(), "Extended bundle should NOT have 'id' in InitCommit");
    assert_eq!(
        Some(temp2.url().clone()),
        init_commit3.from,
        "Extended bundle should have 'from' pointing to parent"
    );

    let base3 = Bundle::open(&temp3.url().to_string(), None).await?;
    assert_eq!(
        base_id,
        base3.id(),
        "Third extended bundle should still have the same ID as base bundle"
    );

    Ok(())
}
