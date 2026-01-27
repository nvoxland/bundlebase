use bundlebase;
use bundlebase::bundle::BundleFacade;
use bundlebase::io::{readable_file_from_url, IOReadWriteDir};
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::{Bundle, BundlebaseError, BundleConfig};
use std::collections::HashMap;
use url::Url;

mod common;

/// Helper to sum total changes from FetchResults
fn total_changes(results: &[bundlebase::source::FetchResults]) -> usize {
    results.iter().map(|r| r.total_count()).sum()
}

/// Helper to create args for remote_dir source function
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
    target_dir: &dyn IOReadWriteDir,
    target_name: &str,
) -> Result<(), BundlebaseError> {
    let source_obj =
        readable_file_from_url(&Url::parse(test_file)?, BundleConfig::default().into())?;
    let data: bytes::Bytes = source_obj
        .read_bytes()
        .await?
        .expect("Failed to read source file");
    let target_file = target_dir.writable_file(target_name)?;
    target_file.write(data).await?;
    Ok(())
}

#[tokio::test]
async fn test_create_source_basic() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define a source with default patterns
    bundle
        .create_source("remote_dir", make_source_args("memory:///some/path/", None), None)
        .await?;

    // Commit and verify
    bundle.commit("Defined source").await?;

    // Verify commit file contains createSource operation
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();
    assert!(contents.contains("type: createSource"));
    assert!(contents.contains("url: memory:///some/path/"));

    // Reopen and verify source persists (bundle opens successfully)
    let _loaded = Bundle::open(data_dir.as_str(), None).await?;

    Ok(())
}

#[tokio::test]
async fn test_create_source_with_patterns() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define source with specific patterns
    bundle
        .create_source(
            "remote_dir",
            make_source_args("memory:///data/", Some("**/*.parquet,**/*.csv")),
            None,
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Verify patterns are serialized correctly in args (as comma-separated string)
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();
    assert!(contents.contains("patterns: '**/*.parquet,**/*.csv'"));

    Ok(())
}

#[tokio::test]
async fn test_create_source_default_patterns() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Define source without patterns (function defaults to **/* internally)
    bundle
        .create_source("remote_dir", make_source_args("memory:///data/", None), None)
        .await?;

    bundle.commit("Defined source").await?;

    // When patterns are not provided, they are not included in args
    // The remote_dir function defaults to "**/*" internally
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();
    assert!(contents.contains("type: createSource"));
    assert!(contents.contains("url: memory:///data/"));
    // Patterns are not in args when not explicitly provided
    assert!(!contents.contains("patterns:"));

    Ok(())
}

#[tokio::test]
async fn test_create_source_auto_attaches_files() -> Result<(), BundlebaseError> {
    // Create a source directory with test files
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test data to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
        .await?;

    // Verify file was auto-attached (create_source calls fetch automatically)
    assert_eq!(bundle.num_rows().await?, 1000);

    // Verify subsequent fetch finds nothing new
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 0);

    Ok(())
}

#[tokio::test]
async fn test_fetch_attaches_new_files() -> Result<(), BundlebaseError> {
    // Create a source directory
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Create bundle and define source (empty directory)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
        .await?;

    // Verify no data yet by fetching (should attach nothing)
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 0);

    // Now add a file to the source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Fetch should find and attach the new file
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 1);

    // Verify data is now available
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_fetch_idempotent() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test data to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source (auto-attaches)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
        .await?;

    // First explicit fetch should find nothing (already attached by create_source)
    let results1 = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results1), 0);

    // Second fetch should also find nothing
    let results2 = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results2), 0);

    // Data should still be there
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_fetch_incremental() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy first file to source
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*")), None)
        .await?;

    // First file should be auto-attached
    assert_eq!(bundle.num_rows().await?, 1000);

    // Add second file
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        source_dir.as_ref(),
        "customers.csv",
    )
    .await?;

    // Fetch should only attach the new file
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 1);

    Ok(())
}

#[tokio::test]
async fn test_pattern_filtering() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy parquet file
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Copy CSV file
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        source_dir.as_ref(),
        "customers.csv",
    )
    .await?;

    // Create bundle with parquet-only pattern
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
        .await?;

    // Only parquet should be attached (1000 rows)
    assert_eq!(bundle.num_rows().await?, 1000);

    // Fetch should not find CSV (doesn't match pattern)
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 0);

    Ok(())
}

#[tokio::test]
async fn test_source_persists_after_commit() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle, define source, commit
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
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
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
        .await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains source in attach operation
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // The attach operation should have a source field
    assert!(contents.contains("source:"), "AttachBlock should have source: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_create_source_serialization() -> Result<(), BundlebaseError> {
    let bundle_dir = random_memory_dir();
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args("memory:///data/", Some("**/*.parquet")), None)
        .await?;

    bundle.commit("Defined source").await?;

    // Read the commit file and verify CreateSource is serialized
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    assert!(contents.contains("type: createSource"));
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
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle, define source, commit
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir1.url().as_str(), None).await?;

    bundle
        .create_source("remote_dir", make_source_args(source_dir.url().as_str(), Some("**/*.parquet")), None)
        .await?;

    bundle.commit("Defined source").await?;

    // Extend to new location
    let loaded = Bundle::open(bundle_dir1.url().as_str(), None).await?;
    let mut extended = loaded.extend(Some(bundle_dir2.url().as_str()))?;

    // Add a new file to source
    copy_test_file(
        test_datafile("customers-0-100.csv"),
        source_dir.as_ref(),
        "customers.csv",
    )
    .await?;

    // Extended bundle should be able to fetch from the source
    // But only CSV matches since we defined pattern as **/*
    // Actually, the pattern is **/*.parquet, so CSV won't match
    let results = extended.fetch_all().await?;
    assert_eq!(total_changes(&results), 0); // CSV doesn't match parquet pattern

    extended.commit("Extended").await?;

    Ok(())
}

#[tokio::test]
async fn test_create_source_copy_default() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source (default is copy=true)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source(
            "remote_dir",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
            None,
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains attach operation with location in bundle data_dir
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // The location should be in the bundle data_dir, not the original source
    // And source should contain the original location
    assert!(contents.contains("source:"), "AttachBlock should have source: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_create_source_copy_false() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source with copy=false
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    let mut args = make_source_args(source_dir.url().as_str(), Some("**/*.parquet"));
    args.insert("copy".to_string(), "false".to_string());

    bundle.create_source("remote_dir", args, None).await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains attach operation with location at original source
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // The location should be the original source URL (not copied)
    assert!(contents.contains(source_dir.url().as_str()),
        "AttachBlock location should reference original source: {}", contents);

    Ok(())
}

#[tokio::test]
async fn test_create_source_copy_true_explicit() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source with explicit copy=true
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    let mut args = make_source_args(source_dir.url().as_str(), Some("**/*.parquet"));
    args.insert("copy".to_string(), "true".to_string());

    bundle.create_source("remote_dir", args, None).await?;

    bundle.commit("Defined source").await?;

    // Verify commit file contains attach operation with location in bundle data_dir
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // The location should be in the bundle data_dir (copied)
    // And source should contain the original location
    assert!(contents.contains("source:"), "AttachBlock should have source: {}", contents);

    // Data should be queryable
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_create_source_creates_single_change() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    // Record change count before create_source (create may add a change)
    let changes_before = bundle.status().changes().len();

    bundle
        .create_source(
            "remote_dir",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
            None,
        )
        .await?;

    // After create_source, should have exactly 1 additional change (not multiple)
    // This change should contain both the CreateSourceOp and the AttachBlockOp
    let status = bundle.status();
    let changes = status.changes();
    let changes_added = changes.len() - changes_before;
    assert_eq!(
        changes_added, 1,
        "create_source should create exactly 1 change, got {}. Changes: {:?}",
        changes_added,
        changes.iter().map(|c| &c.description).collect::<Vec<_>>()
    );

    // The create_source change should contain multiple operations (CreateSourceOp + AttachBlockOp)
    let create_source_change = &changes[changes_before];
    let ops_count = create_source_change.operations.len();
    assert!(
        ops_count >= 2,
        "The change should contain at least 2 operations (CreateSourceOp + AttachBlockOp), got {}",
        ops_count
    );

    // Verify the data is actually attached
    assert_eq!(bundle.num_rows().await?, 1000);

    Ok(())
}

#[tokio::test]
async fn test_source_location_uses_relative_path() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source(
            "remote_dir",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
            None,
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Read the commit file and verify the sourceLocation is a relative path
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // Find the attachBlock operation and verify its sourceLocation field
    // The YAML format is now nested:
    // source:
    //   id: '...'
    //   location: userdata.parquet
    //   version: '...'
    let lines: Vec<&str> = contents.lines().collect();
    let mut found_attach_block = false;
    let mut in_source_block = false;
    let mut source_location_is_relative = false;

    for line in lines.iter() {
        if line.contains("type: attachBlock") {
            found_attach_block = true;
        }
        // Look for source: field after attachBlock
        if found_attach_block && line.trim() == "source:" {
            in_source_block = true;
            continue;
        }
        // Look for location: within source block (indented)
        if in_source_block && line.trim_start().starts_with("location:") {
            let location_value = line.split(':').skip(1).collect::<Vec<_>>().join(":");
            let location = location_value.trim().trim_matches('\'').trim_matches('"');

            // A relative path should NOT contain "://" (URL scheme indicator)
            // and should be just the filename like "userdata.parquet"
            source_location_is_relative = !location.contains("://");

            // The relative path should be just the filename
            assert_eq!(
                location, "userdata.parquet",
                "source.location should be relative path 'userdata.parquet', got: {}",
                location
            );

            break;
        }
        // Exit source block when we hit a line that's not indented enough
        if in_source_block && !line.starts_with("      ") && !line.trim().is_empty() {
            in_source_block = false;
        }
    }

    assert!(found_attach_block, "Should have attachBlock operation in commit");
    assert!(
        source_location_is_relative,
        "source.location should be relative path, not URL. Contents:\n{}",
        contents
    );

    Ok(())
}

#[tokio::test]
async fn test_copy_true_uses_relative_path() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source with copy=true (default)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source(
            "remote_dir",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
            None,
        )
        .await?;

    bundle.commit("Defined source").await?;

    // Read the commit file and verify the attach location is a relative path
    let (contents, _, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // The attachBlock location should be a relative path like "ab/cdef12345.parquet"
    // NOT a full URL like "memory:///xxx/ab/cdef12345.parquet"

    // Find the attachBlock operation and verify its location field
    // The YAML format is: location: <path>
    let lines: Vec<&str> = contents.lines().collect();
    let mut found_attach_block = false;
    let mut location_is_relative = false;

    for line in lines.iter() {
        if line.contains("type: attachBlock") {
            found_attach_block = true;
        }
        // Look for location field after attachBlock
        if found_attach_block && line.trim_start().starts_with("location:") {
            let location_value = line.split(':').skip(1).collect::<Vec<_>>().join(":");
            let location = location_value.trim().trim_matches('\'').trim_matches('"');

            // A relative path should NOT contain "://" (URL scheme indicator)
            // and should be a simple path like "ab/cdef12345678.parquet"
            location_is_relative = !location.contains("://");

            // Also verify it looks like a SHA-based path (2 char dir / hash.ext)
            let parts: Vec<&str> = location.split('/').collect();
            assert!(
                parts.len() >= 2 && parts[0].len() == 2,
                "Location should be SHA-based path like 'ab/cdef12345.parquet', got: {}",
                location
            );

            break;
        }
    }

    assert!(found_attach_block, "Should have attachBlock operation in commit");
    assert!(
        location_is_relative,
        "attachBlock location should be relative path, not URL. Contents:\n{}",
        contents
    );

    Ok(())
}

#[tokio::test]
async fn test_fetch_with_copy_no_duplicates() -> Result<(), BundlebaseError> {
    let source_dir = random_memory_dir();
    let bundle_dir = random_memory_dir();

    // Copy test file to source directory
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata.parquet",
    )
    .await?;

    // Create bundle and define source (default copy=true)
    let mut bundle =
        bundlebase::BundleBuilder::create(bundle_dir.url().as_str(), None).await?;

    bundle
        .create_source(
            "remote_dir",
            make_source_args(source_dir.url().as_str(), Some("**/*.parquet")),
            None,
        )
        .await?;

    // File should be auto-attached (create_source calls fetch)
    assert_eq!(bundle.num_rows().await?, 1000);

    // Subsequent fetch should not re-copy the file
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 0, "Should not re-attach already copied file");

    // Add a second parquet file (same data, different name)
    copy_test_file(
        test_datafile("userdata.parquet"),
        source_dir.as_ref(),
        "userdata2.parquet",
    )
    .await?;

    // Fetch should only find the new file
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 1, "Should attach only the new file");

    // Now we should have 2000 rows (1000 from each file)
    assert_eq!(bundle.num_rows().await?, 2000);

    // Another fetch should find nothing
    let results = bundle.fetch_all().await?;
    assert_eq!(total_changes(&results), 0, "Should not re-attach already copied files");

    Ok(())
}
