use bundlebase::bundle::{BundleBuilder, BundleFacade};
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::{Bundle, IndexType};
use tempfile::TempDir;

/// Tests exporting a bundle to tar and reopening it
#[tokio::test]
async fn test_export_and_reopen_tar() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("test.tar");

    // Create bundle in memory
    let mut bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial data").await.unwrap();

    // Export to tar
    bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    // Verify tar file exists
    assert!(tar_path.exists(), "Tar file should be created");

    // Open from tar
    let tar_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let tar_history = tar_bundle.history();
    assert_eq!(tar_history.len(), 1, "Should have 1 commit in history");

    // Verify data can be queried
    let count = tar_bundle.num_rows().await.unwrap();
    assert!(count > 0, "Should be able to query data from tar bundle");
}

/// Tests committing to a tar bundle (append mode)
#[tokio::test]
async fn test_commit_to_tar() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("appendable.tar");

    // Create and export
    let mut bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("v1").await.unwrap();
    bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    // Open tar and make changes
    let opened_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let mut tar_builder = BundleBuilder::extend(opened_bundle.into(), None).unwrap();
    tar_builder.filter("id > 100", vec![]).await.unwrap();
    tar_builder.commit("v2 - filtered").await.unwrap();

    // Reopen and verify both commits exist
    let reopened = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let reopened_history = reopened.history();
    assert_eq!(reopened_history.len(), 2, "Should have 2 commits in history");
    assert_eq!(reopened_history[0].message, "v1");
    assert_eq!(reopened_history[1].message, "v2 - filtered");
}

/// Tests multiple commits to tar bundle
#[tokio::test]
async fn test_multiple_commits_to_tar() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("multi_commit.tar");

    // Create initial bundle and export
    let mut bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("v1").await.unwrap();
    bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    // Make multiple commits to the tar file
    for i in 2..=5 {
        let opened = Bundle::open(tar_path.to_str().unwrap(), None)
            .await
            .unwrap();
        let mut builder = BundleBuilder::extend(opened.into(), None).unwrap();
        builder
            .filter(&format!("id > {}", i * 50), vec![])
            .await
            .unwrap();
        builder.commit(&format!("v{}", i)).await.unwrap();
    }

    // Verify all commits exist
    let final_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let final_history = final_bundle.history();
    assert_eq!(final_history.len(), 5, "Should have 5 commits in history");
}

/// Tests that tar bundle preserves metadata correctly
#[tokio::test]
async fn test_tar_preserves_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("metadata.tar");

    // Create bundle with metadata
    let mut bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    bundle.set_name("Test Bundle").await.unwrap();
    bundle.set_description("A test bundle for tar export").await.unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("Initial commit").await.unwrap();

    // Export to tar
    bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    // Open from tar and verify metadata
    let tar_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(tar_bundle.name(), Some("Test Bundle".to_string()));
    assert_eq!(tar_bundle.description(), Some("A test bundle for tar export".to_string()));
}

/// Tests creating an index in a tar bundle
#[tokio::test]
async fn test_create_index_in_tar() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("with_index.tar");

    // Create bundle and export
    let mut bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("v1").await.unwrap();
    bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    // Open tar and create index
    let opened = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let mut builder = BundleBuilder::extend(opened.into(), None).unwrap();
    builder.create_index("id", IndexType::Column).await.unwrap();
    builder.commit("v2 - added index").await.unwrap();

    // Reopen and verify index exists
    let final_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let final_history = final_bundle.history();
    assert_eq!(final_history.len(), 2, "Should have 2 commits in history");

    // Verify we can query data (index should work)
    let final_count = final_bundle.num_rows().await.unwrap();
    assert!(final_count > 0);
}

/// Tests that querying works the same on tar bundles as regular bundles
#[tokio::test]
async fn test_tar_query_equivalence() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("query_test.tar");

    // Create and query memory bundle
    let mut mem_bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    mem_bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    mem_bundle.filter("SELECT * FROM bundle WHERE id > 100", vec![]).await.unwrap();
    mem_bundle.commit("filtered").await.unwrap();

    let mem_count = mem_bundle.bundle().num_rows().await.unwrap();

    // Export to tar and query
    mem_bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    let tar_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let tar_count = tar_bundle.num_rows().await.unwrap();

    // Verify same results
    assert_eq!(mem_count, tar_count, "Tar bundle should return same query results as memory bundle");
}

/// Tests exporting from a Bundle (read-only) instance
#[tokio::test]
async fn test_export_from_bundle() {
    let temp_dir = TempDir::new().unwrap();
    let memory_url = random_memory_url();
    let tar_path = temp_dir.path().join("from_bundle.tar");

    // Create and commit a bundle
    let mut builder = BundleBuilder::create(memory_url.as_str(), None).await.unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    builder.commit("Initial data").await.unwrap();

    // Open as read-only Bundle
    let bundle = Bundle::open(memory_url.as_str(), None).await.unwrap();

    // Export should work from Bundle
    let result = bundle.export_tar(tar_path.to_str().unwrap()).await;
    assert!(result.is_ok(), "Should be able to export from Bundle");

    // Verify tar file was created
    assert!(tar_path.exists(), "Tar file should be created");

    // Verify we can reopen it
    let reopened = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(reopened.history().len(), 1);
}

/// Tests exporting from BundleBuilder with no uncommitted changes
#[tokio::test]
async fn test_export_from_builder_no_changes() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("from_builder.tar");

    // Create and commit
    let mut builder = BundleBuilder::create(random_memory_url().as_str(), None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    builder.commit("Initial data").await.unwrap();

    // Export should work when there are no uncommitted changes
    let result = builder.export_tar(tar_path.to_str().unwrap()).await;
    assert!(
        result.is_ok(),
        "Should be able to export from BundleBuilder with no uncommitted changes"
    );

    // Verify tar file was created
    assert!(tar_path.exists(), "Tar file should be created");
}

/// Tests that exporting from BundleBuilder with uncommitted changes fails
#[tokio::test]
async fn test_export_from_builder_with_uncommitted_changes() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("should_fail.tar");

    // Create bundle with uncommitted changes
    let mut builder = BundleBuilder::create(random_memory_url().as_str(), None)
        .await
        .unwrap();
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    // Note: NOT committing here

    // Export should fail with uncommitted changes
    let result = builder.export_tar(tar_path.to_str().unwrap()).await;
    assert!(
        result.is_err(),
        "Should NOT be able to export with uncommitted changes"
    );

    // Verify error message
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("uncommitted changes"),
        "Error should mention uncommitted changes, got: {}",
        err_msg
    );

    // Verify tar file was NOT created
    assert!(!tar_path.exists(), "Tar file should not be created");
}

/// Tests that file listing works correctly on tar bundles
#[tokio::test]
async fn test_tar_file_listing() {
    let temp_dir = TempDir::new().unwrap();
    let tar_path = temp_dir.path().join("listing.tar");

    // Create bundle with data
    let mut bundle = BundleBuilder::create(random_memory_url().as_str(), None).await.unwrap();
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await
        .unwrap();
    bundle.commit("v1").await.unwrap();

    // Export to tar
    bundle
        .export_tar(tar_path.to_str().unwrap())
        .await
        .unwrap();

    // Open tar and list files
    let tar_bundle = Bundle::open(tar_path.to_str().unwrap(), None)
        .await
        .unwrap();

    // Verify we can access data_dir and list files
    let files = tar_bundle.data_dir().list_files().await.unwrap();
    assert!(
        !files.is_empty(),
        "Should be able to list files in tar bundle"
    );

    // Verify _bundlebase directory exists
    let manifest_count = files
        .iter()
        .filter(|f| f.url.as_str().contains("_bundlebase"))
        .count();
    assert!(
        manifest_count > 0,
        "Should have manifest files in _bundlebase directory"
    );
}
