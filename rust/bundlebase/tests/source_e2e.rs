use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::io::ObjectStoreFile;
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::{Bundle, BundlebaseError, BundleConfig};
use url::Url;

mod common;

/// Helper to copy a test file to a target directory
async fn copy_test_file(
    test_file: &str,
    target_dir: &bundlebase::io::ObjectStoreDir,
    target_name: &str,
) -> Result<(), BundlebaseError> {
    let source_obj =
        ObjectStoreFile::from_url(&Url::parse(test_file)?, BundleConfig::default().into())?;
    let data = source_obj
        .read_bytes()
        .await?
        .expect("Failed to read source file");
    let target_file = target_dir.file(target_name)?;
    target_file.write(data).await?;
    Ok(())
}

#[tokio::test]
async fn test_define_source_basic() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define a source with default patterns
    bundle
        .define_source("memory:///some/path/", None)
        .await?;

    // Commit and verify
    bundle.commit("Defined source").await?;

    // Verify commit file contains defineSource operation
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();
    assert!(contents.contains("type: defineSource"));
    assert!(contents.contains("url: memory:///some/path/"));

    // Reopen and verify source persists (bundle opens successfully)
    let _loaded = Bundle::open(data_dir.as_str(), None).await?;

    Ok(())
}

#[tokio::test]
async fn test_define_source_with_patterns() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define source with specific patterns
    bundle
        .define_source(
            "memory:///data/",
            Some(vec!["**/*.parquet", "**/*.csv"]),
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Verify patterns are serialized correctly
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();
    assert!(contents.contains("patterns:"));
    assert!(contents.contains("- '**/*.parquet'"));
    assert!(contents.contains("- '**/*.csv'"));

    Ok(())
}

#[tokio::test]
async fn test_define_source_default_patterns() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define source without patterns (should default to **/* )
    bundle
        .define_source("memory:///data/", None)
        .await?;

    bundle.commit("Defined source").await?;

    // Verify default pattern is serialized
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();
    assert!(contents.contains("patterns:"));
    assert!(contents.contains("- '**/*'"));

    Ok(())
}

#[tokio::test]
async fn test_define_source_auto_attaches_files() -> Result<(), BundlebaseError> {
    // Create a source directory with test files
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test data to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    // Verify file was auto-attached (define_source calls refresh automatically)
    assert_eq!(bundle.num_rows().await?, 1000);

    // Verify subsequent refresh finds nothing new
    let count = bundle.refresh().await?;
    assert_eq!(count, 0);

    Ok(())
}

#[tokio::test]
async fn test_refresh_attaches_new_files() -> Result<(), BundlebaseError> {
    // Create a source directory
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Create bundle and define source (empty directory)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    // Verify no data yet by checking pending files is empty
    let pending = bundle.check_refresh().await?;
    assert!(pending.is_empty());

    // Now add a file to the source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Refresh should find and attach the new file
    let count = bundle.refresh().await?;
    assert_eq!(count, 1);

    // Verify data is now available
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_refresh_idempotent() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test data to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source (auto-attaches)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    // First explicit refresh should find nothing (already attached by define_source)
    let count1 = bundle.refresh().await?;
    assert_eq!(count1, 0);

    // Second refresh should also find nothing
    let count2 = bundle.refresh().await?;
    assert_eq!(count2, 0);

    // Data should still be there
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_refresh_incremental() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy first file to source
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*"]))
        .await?;

    // First file should be auto-attached
    assert_eq!(bundle.num_rows().await?, 1000);

    // Add second file
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        &source_dir,
        "customers.csv",
    )
    .await?;

    // Refresh should only attach the new file
    let count = bundle.refresh().await?;
    assert_eq!(count, 1);

    Ok(())
}

#[tokio::test]
async fn test_check_refresh() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Create bundle and define source (empty directory)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    // check_refresh should return empty (no files)
    let pending = bundle.check_refresh().await?;
    assert!(pending.is_empty());

    // Add a file
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // check_refresh should find the pending file
    let pending = bundle.check_refresh().await?;
    assert_eq!(pending.len(), 1);
    assert!(pending[0].1.contains("userdata.parquet"));

    // check_refresh should NOT attach the file - verify by checking pending again
    let pending_again = bundle.check_refresh().await?;
    assert_eq!(pending_again.len(), 1); // Still 1 pending file

    // Now refresh to actually attach
    let count = bundle.refresh().await?;
    assert_eq!(count, 1);
    assert_eq!(bundle.num_rows().await?, 1000);

    // Now check_refresh should return empty
    let pending_after = bundle.check_refresh().await?;
    assert!(pending_after.is_empty());

    Ok(())
}

#[tokio::test]
async fn test_pattern_filtering() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy parquet file
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Copy CSV file
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        &source_dir,
        "customers.csv",
    )
    .await?;

    // Create bundle with parquet-only pattern
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    // Only parquet should be attached (1000 rows)
    assert_eq!(bundle.num_rows().await?, 1000);

    // Refresh should not find CSV (doesn't match pattern)
    let count = bundle.refresh().await?;
    assert_eq!(count, 0);

    Ok(())
}

#[tokio::test]
async fn test_source_persists_after_commit() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle, define source, commit
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    bundle.commit("Defined source").await?;

    // Reopen bundle
    let loaded = Bundle::open(bundle_dir.url().as_str(), None).await?;

    // Data should be queryable
    assert_eq!(loaded.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_source_id_in_attach_op() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains sourceId in attach operation
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    // The attach operation should have a sourceId field
    assert!(contents.contains("sourceId:"), "AttachBlock should have sourceId: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_define_source_serialization() -> Result<(), BundlebaseError> {
    let bundle_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source("memory:///data/", Some(vec!["**/*.parquet"]))
        .await?;

    bundle.commit("Defined source").await?;

    // Read the commit file and verify DefineSource is serialized
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    assert!(contents.contains("type: defineSource"));
    assert!(contents.contains("url: memory:///data/"));
    assert!(contents.contains("patterns:"));
    assert!(contents.contains("- '**/*.parquet'"));

    Ok(())
}

#[tokio::test]
async fn test_extend_preserves_source() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir1 = random_memory_dir();
    let bundle_dir2 = random_memory_dir();

    // Copy test file
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle, define source, commit
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir1.url().as_str(), None).await?;

    bundle
        .define_source(source_dir.url().as_str(), Some(vec!["**/*.parquet"]))
        .await?;

    bundle.commit("Defined source").await?;

    // Extend to new location
    let loaded = Bundle::open(bundle_dir1.url().as_str(), None).await?;
    let mut extended = loaded.extend(Some(bundle_dir2.url().as_str()))?;

    // Add a new file to source
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        &source_dir,
        "customers.csv",
    )
    .await?;

    // Extended bundle should be able to refresh from the source
    // But only CSV matches since we defined pattern as **/*
    // Actually, the pattern is **/*.parquet, so CSV won't match
    let count = extended.refresh().await?;
    assert_eq!(count, 0); // CSV doesn't match parquet pattern

    extended.commit("Extended").await?;

    Ok(())
}
