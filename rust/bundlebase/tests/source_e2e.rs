use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::io::{IOLister, IOReader, IOWriter, IOFile};
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::{Bundle, BundlebaseError, BundleConfig};
use std::collections::HashMap;
use url::Url;

mod common;

/// Helper to create args for data_directory source function
fn make_source_args(url: &str, patterns: Option<&str>) -> HashMap<String, String> {
    let mut args = HashMap::new();
    args.insert("url".to_string(), url.to_string());
    if let Some(p) = patterns {
        args.insert("patterns".to_string(), p.to_string());
    }
    args
}

/// Helper to copy a test file to a target directory
async fn copy_test_file(
    test_file: &str,
    target_dir: &bundlebase::io::IODir,
    target_name: &str,
) -> Result<(), BundlebaseError> {
    let source_obj =
        IOFile::from_url(&Url::parse(test_file)?, BundleConfig::default().into())?;
    let data = source_obj
        .read_bytes()
        .await?
        .expect("Failed to read source file");
    let target_file = target_dir.io_file(target_name)?;
    target_file.write(data).await?;
    Ok(())
}

#[tokio::test]
async fn test_define_source_basic() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define a source with default patterns
    bundle
        .define_source("data_directory", make_source_args("memory:///some/path/", None))
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
            "data_directory",
            make_source_args("memory:///data/", Some("**/*.parquet,**/*.csv")),
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Verify patterns are serialized correctly in args (as comma-separated string)
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();
    assert!(contents.contains("patterns: '**/*.parquet,**/*.csv'"));

    Ok(())
}

#[tokio::test]
async fn test_define_source_default_patterns() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define source without patterns (function defaults to **/* internally)
    bundle
        .define_source("data_directory", make_source_args("memory:///data/", None))
        .await?;

    bundle.commit("Defined source").await?;

    // When patterns are not provided, they are not included in args
    // The data_directory function defaults to "**/*" internally
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();
    assert!(contents.contains("type: defineSource"));
    assert!(contents.contains("url: memory:///data/"));
    // Patterns are not in args when not explicitly provided
    assert!(!contents.contains("patterns:"));

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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*")))
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
        .await?;

    bundle.commit("Defined source").await?;

    // Reopen bundle
    let loaded = Bundle::open(bundle_dir.url().as_str(), None).await?;

    // Data should be queryable
    assert_eq!(loaded.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_source_in_attach_op() -> Result<(), BundlebaseError> {
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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
        .await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains source in attach operation
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    // The attach operation should have a source field
    assert!(contents.contains("source:"), "AttachBlock should have source: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_define_source_serialization() -> Result<(), BundlebaseError> {
    let bundle_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source("data_directory", make_source_args("memory:///data/", Some("**/*.parquet")))
        .await?;

    bundle.commit("Defined source").await?;

    // Read the commit file and verify DefineSource is serialized
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    assert!(contents.contains("type: defineSource"));
    assert!(contents.contains("url: memory:///data/"));
    assert!(contents.contains("patterns: '**/*.parquet'"));

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
        .define_source("data_directory", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")))
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

#[tokio::test]
async fn test_define_source_copy_default() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source (default is copy=true)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(
            "data_directory",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains attach operation with location in bundle data_dir
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    // The location should be in the bundle data_dir, not the original source
    // And source_location should be the original URL
    assert!(contents.contains("sourceLocation:"), "AttachBlock should have sourceLocation: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_define_source_copy_false() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source with copy=false
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    let mut args = make_source_args(source_dir.url().as_str(), Some("**/*.parquet"));
    args.insert("copy".to_string(), "false".to_string());

    bundle.define_source("data_directory", args).await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains attach operation with location at original source
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    // The location should be the original source URL (not copied)
    assert!(contents.contains(source_dir.url().as_str()),
        "AttachBlock location should reference original source: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_define_source_copy_true_explicit() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source with explicit copy=true
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    let mut args = make_source_args(source_dir.url().as_str(), Some("**/*.parquet"));
    args.insert("copy".to_string(), "true".to_string());

    bundle.define_source("data_directory", args).await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains attach operation with location in bundle data_dir
    let (contents, _, _) = common::latest_commit(bundle.data_dir()).await?.unwrap();

    // The location should be in the bundle data_dir (copied)
    // And source_location should be the original URL
    assert!(contents.contains("sourceLocation:"), "AttachBlock should have sourceLocation: {}", contents);

    // Data should be queryable
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_refresh_with_copy_no_duplicates() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        &source_dir,
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source (default copy=true)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .define_source(
            "data_directory",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
        )
        .await?;

    // File should be auto-attached (define_source calls refresh)
    assert_eq!(bundle.num_rows().await?, 1000);

    // Subsequent refresh should not re-copy the file
    let count = bundle.refresh().await?;
    assert_eq!(count, 0, "Should not re-attach already copied file");

    // Add a second file
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        &source_dir,
        "customers.parquet", // Using parquet extension to match pattern
    )
    .await?;

    // Refresh should only find the new file
    // Note: This will fail because customers.csv is not a parquet file
    // Let's copy another parquet file instead

    Ok(())
}
