//! E2E tests for git-based version detection on attached source files.
//!
//! These tests run against the full bundle pipeline (attach → commit →
//! reopen → query) and verify that the `system.git_versioning` config
//! gates the new behavior end-to-end:
//!
//! - Off (default): version comes from the storage layer (mtime hash for
//!   local files), no git probe happens.
//! - On: version is the raw git blob OID (40-char sha1 hex, no prefix);
//!   queries succeed across no-op edits and `git add` / `git commit`;
//!   queries fail when the file content changes; attach errors out when
//!   the source isn't inside a git working tree.
//!
//! Each test runs in a fresh tempdir so we don't see the bundlebase repo's
//! own git state.

use bundlebase::bundle::BundleFacade;
use bundlebase::bundle_config::Scope;
use bundlebase::test_utils::random_memory_url;
use bundlebase::Bundle;
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use std::path::Path;
use std::process::Command;
use url::Url;

mod common;

fn init() {
    common::init_catalog();
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git invocation failed to spawn");
    assert!(status.success(), "git {:?} failed in {:?}", args, dir);
}

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet"]);
    run_git(dir, &["config", "user.email", "t@example.com"]);
    run_git(dir, &["config", "user.name", "test"]);
}

fn system_scope() -> Scope {
    Scope::try_from("system").expect("system scope")
}

/// Touch a file with new content while preserving its existence.
fn write(path: &Path, content: &str) {
    std::fs::write(path, content).expect("write file");
}

/// Convert a local path to a `file://` URL string for `attach`.
fn file_url(path: &Path) -> String {
    Url::from_file_path(path)
        .expect("absolute path")
        .to_string()
}

/// Look up the recorded `version` string of the attached block at `location`.
/// Panics if no block is found — failing the lookup means the test set
/// itself up wrong.
fn block_version(bundle: &bundlebase::Bundle, location: &str) -> String {
    bundle
        .find_block_by_current_location(location)
        .unwrap_or_else(|| panic!("no block at {}", location))
        .version()
}

#[tokio::test]
async fn query_succeeds_when_git_oid_unchanged() -> Result<(), BundlebaseError> {
    init();
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n2,Bob\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await?;
    builder.attach(&file_url(&csv), None).await?;
    let rows_before = builder.num_rows().await?;
    assert_eq!(rows_before, 2);
    builder.commit("Initial commit").await?;

    // Re-write the same bytes (mtime changes, content does not). With the
    // git OID being a content hash, the version is unchanged, so reopening
    // and querying should succeed.
    write(&csv, "id,name\n1,Alice\n2,Bob\n");

    let bundle = Bundle::open(bundle_url.as_str(), None).await?;
    let rows_after = bundle.num_rows().await?;
    assert_eq!(rows_after, 2, "no-op rewrite must not invalidate version");

    // Recorded version should be the raw git blob OID — a 40-char lowercase
    // sha1 hex string matching what `git hash-object` produces directly.
    let version = block_version(&bundle, &file_url(&csv));
    assert_eq!(
        version.len(),
        40,
        "expected 40-char sha1 hex, got {:?}",
        version
    );
    assert!(
        version.chars().all(|c| c.is_ascii_hexdigit()),
        "expected hex OID, got {:?}",
        version
    );
    let expected = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["hash-object", csv.to_str().expect("utf8 path")])
        .output()
        .expect("git hash-object");
    let expected_oid = String::from_utf8(expected.stdout)
        .expect("utf8 stdout")
        .trim()
        .to_string();
    assert_eq!(version, expected_oid);
    Ok(())
}

#[tokio::test]
async fn query_succeeds_across_git_add_and_commit() -> Result<(), BundlebaseError> {
    init();
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await?;
    // Attach while the source is still untracked.
    builder.attach(&file_url(&csv), None).await?;
    builder.commit("Initial commit").await?;

    // Now stage and commit the file. The bytes don't change, so the git
    // blob OID is identical → the recorded version still matches.
    run_git(repo.path(), &["add", "data.csv"]);
    run_git(repo.path(), &["commit", "-m", "track csv", "--quiet"]);

    let bundle = Bundle::open(bundle_url.as_str(), None).await?;
    let rows = bundle.num_rows().await?;
    assert_eq!(
        rows, 1,
        "git add/commit must not invalidate version (content unchanged)"
    );
    Ok(())
}

#[tokio::test]
async fn query_fails_when_file_content_changes() -> Result<(), BundlebaseError> {
    init();
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await?;
    builder.attach(&file_url(&csv), None).await?;
    builder.commit("Initial commit").await?;

    // Real content change → OID changes → query must fail.
    write(&csv, "id,name\n1,Alice\n2,Bob\n");

    let bundle = Bundle::open(bundle_url.as_str(), None).await?;
    let result = bundle.num_rows().await;
    let err = result.expect_err("query should fail after content change");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("version"),
        "expected a version-mismatch error, got: {}",
        msg
    );
    Ok(())
}

#[tokio::test]
async fn attach_errors_when_enabled_outside_git_repo() -> Result<(), BundlebaseError> {
    init();
    // Source dir has no git init — opting into git_versioning should fail
    // loudly during attach rather than silently fall back.
    let dir = tempfile::tempdir()?;
    let csv = dir.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await?;
    let result = builder.attach(&file_url(&csv), None).await;
    let Err(err) = result else {
        panic!("attach should fail outside a git working tree");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("system.git_versioning"),
        "error should mention the config key, got: {}",
        msg
    );
    Ok(())
}

#[tokio::test]
async fn enabling_git_versioning_refreshes_existing_block_versions(
) -> Result<(), BundlebaseError> {
    // The change hook for system.git_versioning must walk every attached
    // block and emit UpdateVersionOps so existing recorded versions
    // (mtime hashes) get replaced with git OIDs in the same commit.
    init();
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n2,Bob\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    // Attach FIRST, while git_versioning is off — version recorded as mtime hash.
    builder.attach(&file_url(&csv), None).await?;
    let pre_version = block_version(builder.bundle(), &file_url(&csv));
    assert_ne!(
        pre_version.len(),
        40,
        "before flip, version should not look like a git OID; got {:?}",
        pre_version
    );

    // Now flip the flag. The hook must refresh the recorded version.
    builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await?;

    let post_version = block_version(builder.bundle(), &file_url(&csv));
    let expected = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["hash-object", csv.to_str().expect("utf8 path")])
        .output()
        .expect("git hash-object");
    let expected_oid = String::from_utf8(expected.stdout)
        .expect("utf8 stdout")
        .trim()
        .to_string();
    assert_eq!(
        post_version, expected_oid,
        "hook should have updated the recorded version to the git OID"
    );

    // Commit and reopen — the refreshed version must persist.
    builder.commit("Enable git versioning").await?;
    let bundle = Bundle::open(bundle_url.as_str(), None).await?;
    assert_eq!(bundle.num_rows().await?, 2);
    let reopened_version = block_version(&bundle, &file_url(&csv));
    assert_eq!(reopened_version, expected_oid);
    Ok(())
}

#[tokio::test]
async fn disabling_git_versioning_refreshes_existing_block_versions(
) -> Result<(), BundlebaseError> {
    // Symmetric to the enabling test: flipping the flag from on → off
    // must refresh recorded versions back to the storage-layer value
    // (mtime hash for local files), not leave stale git OIDs around.
    init();
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await?;
    builder.attach(&file_url(&csv), None).await?;
    let on_version = block_version(builder.bundle(), &file_url(&csv));
    assert_eq!(on_version.len(), 40, "should be git OID while flag is on");

    // Flip off — hook must replace the git OID with whatever the storage
    // layer reports for this file (mtime-derived hash, not 40-char hex).
    builder
        .save_config(&system_scope(), "git_versioning", "false")
        .await?;
    let off_version = block_version(builder.bundle(), &file_url(&csv));
    assert_ne!(
        off_version, on_version,
        "disabling git_versioning must refresh recorded version"
    );
    assert_ne!(
        off_version.len(),
        40,
        "after disabling, version should not look like a git OID; got {:?}",
        off_version
    );
    Ok(())
}

#[tokio::test]
async fn hook_error_aborts_save_config() -> Result<(), BundlebaseError> {
    // If the change hook fails (e.g. an attached source file has been
    // removed and so version() can't be recomputed under the new policy),
    // the surrounding save_config call must error out rather than silently
    // leaving the manifest in a half-updated state.
    init();
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(&file_url(&csv), None).await?;

    // Delete the source out from under us. The hook walks every attached
    // block and calls version() on each, so this is the failure path.
    std::fs::remove_file(&csv)?;

    let result = builder
        .save_config(&system_scope(), "git_versioning", "true")
        .await;
    assert!(
        result.is_err(),
        "save_config should fail when its change hook fails"
    );
    Ok(())
}

#[tokio::test]
async fn default_config_does_not_use_git() -> Result<(), BundlebaseError> {
    init();
    // Even inside a working tree, the default config (no system.git_versioning)
    // must not produce git-prefixed versions. Smoke-test by attaching and
    // querying — we don't check the exact version string here, just that
    // the default flow works without error and returns expected rows.
    let repo = tempfile::tempdir()?;
    init_repo(repo.path());
    let csv = repo.path().join("data.csv");
    write(&csv, "id,name\n1,Alice\n2,Bob\n3,Carol\n");

    let bundle_url = random_memory_url();
    let builder = bundlebase::BundleBuilder::create(bundle_url.as_str(), None).await?;
    builder.attach(&file_url(&csv), None).await?;
    builder.commit("Initial commit").await?;

    let bundle = Bundle::open(bundle_url.as_str(), None).await?;
    let rows = bundle.num_rows().await?;
    assert_eq!(rows, 3);

    // Without the flag, version must not be the git OID even though the
    // file lives in a git working tree. Compare against `git hash-object`
    // output directly: the recorded version should differ.
    let version = block_version(&bundle, &file_url(&csv));
    let expected = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["hash-object", csv.to_str().expect("utf8 path")])
        .output()
        .expect("git hash-object");
    let oid = String::from_utf8(expected.stdout)
        .expect("utf8 stdout")
        .trim()
        .to_string();
    assert_ne!(
        version, oid,
        "default config must not consult git, got OID-equal version {:?}",
        version
    );
    Ok(())
}
