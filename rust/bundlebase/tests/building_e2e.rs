use bundlebase;
use bundlebase::bundle::{BundleFacade, InitCommit, INIT_FILENAME};
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::BundlebaseError;
use bundlebase::Bundle;

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
