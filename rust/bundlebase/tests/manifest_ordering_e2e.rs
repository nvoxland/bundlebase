//! E2E tests for manifest numbering and replay order.
//!
//! Manifest names are `{5-digit number}{12-char hash}.yaml`. Concurrent
//! writers may produce the same number, so replay order must be the full
//! filename order on every backend, and each commit must take its number
//! from a fresh listing of the manifest directory rather than a counter
//! captured when the session opened.

use bundlebase::bundle::{BundleFacade, INIT_FILENAME};
use bundlebase::test_utils::{random_memory_url, test_config};
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use bundlebase_io::IOReadWriteDir;
use rstest::rstest;
use std::sync::Arc;

mod common;

fn init() {
    common::init_catalog();
}

/// Where a test bundle lives. The temp dir is kept alive alongside the path.
struct Location {
    path: String,
    _tmp: Option<tempfile::TempDir>,
}

fn location(backend: &str) -> Location {
    match backend {
        "memory" => Location {
            path: random_memory_url().to_string(),
            _tmp: None,
        },
        "local" => {
            let tmp = tempfile::tempdir().unwrap();
            Location {
                path: tmp.path().to_str().unwrap().to_string(),
                _tmp: Some(tmp),
            }
        }
        other => panic!("unknown backend {other}"),
    }
}

async fn meta_dir(path: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError> {
    bundlebase_io::writable_dir_from_str(path, test_config())
        .await?
        .writable_subdir("_bundlebase")
}

/// Manifest filenames directly in `_bundlebase/`, excluding the init file, sorted.
async fn manifest_names(path: &str) -> Result<Vec<String>, BundlebaseError> {
    let dir = meta_dir(path).await?;
    let prefix = dir.url().to_string();
    let mut names = dir
        .list_files()
        .await?
        .into_iter()
        .filter(|f| {
            let rel = f.url.as_str()[prefix.len()..].trim_start_matches('/');
            !rel.contains('/') && f.filename() != Some(INIT_FILENAME)
        })
        .filter_map(|f| f.filename().map(str::to_string))
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn number(name: &str) -> u32 {
    name[0..5].parse().unwrap()
}

async fn new_names(path: &str, before: &[String]) -> Result<Vec<String>, BundlebaseError> {
    Ok(manifest_names(path)
        .await?
        .into_iter()
        .filter(|n| !before.contains(n))
        .collect())
}

/// Opens the committed bundle at `path` as an independent writer.
async fn writer(path: &str) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    BundleBuilder::extend(Bundle::open(path, None).await?, None).await
}

async fn read_manifest(path: &str, name: &str) -> Result<bytes::Bytes, BundlebaseError> {
    let dir = meta_dir(path).await?;
    Ok(dir
        .file(name)?
        .read_bytes()
        .await?
        .expect("manifest exists"))
}

async fn write_manifest(path: &str, name: &str, data: bytes::Bytes) -> Result<(), BundlebaseError> {
    meta_dir(path).await?.writable_file(name)?.write(data).await
}

async fn delete_manifest(path: &str, name: &str) -> Result<(), BundlebaseError> {
    meta_dir(path).await?.writable_file(name)?.delete().await
}

/// Two manifests with the same number replay in full-filename order, whatever
/// order they were created in.
#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn test_same_number_replays_in_filename_order(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    init();
    let loc = location(backend);
    let path = loc.path.as_str();

    let builder = BundleBuilder::create(path, None).await?;
    builder.set_name("base").await?;
    builder.commit("initial").await?;
    let base = manifest_names(path).await?;

    // Writer A commits, then its manifest is set aside so writer B, listing
    // the directory, picks the same number.
    let a = writer(path).await?;
    a.set_name("alpha").await?;
    a.commit("alpha").await?;
    let a_name = new_names(path, &base).await?.remove(0);
    let a_data = read_manifest(path, &a_name).await?;
    delete_manifest(path, &a_name).await?;

    let b = writer(path).await?;
    b.set_name("beta").await?;
    b.commit("beta").await?;
    let b_name = new_names(path, &base).await?.remove(0);
    let b_data = read_manifest(path, &b_name).await?;

    assert_eq!(
        number(&a_name),
        number(&b_name),
        "both writers saw the same listing"
    );
    assert_ne!(a_name, b_name);

    // The later filename replays last, so its name wins.
    let expected = if a_name > b_name { "alpha" } else { "beta" };

    // Creation order 1: B then A.
    write_manifest(path, &a_name, a_data.clone()).await?;
    let first = Bundle::open(path, None).await?.name();

    // Creation order 2: A then B.
    delete_manifest(path, &a_name).await?;
    delete_manifest(path, &b_name).await?;
    write_manifest(path, &a_name, a_data).await?;
    write_manifest(path, &b_name, b_data).await?;
    let second = Bundle::open(path, None).await?.name();

    assert_eq!(first.as_deref(), Some(expected));
    assert_eq!(second.as_deref(), Some(expected));
    Ok(())
}

/// A commit takes its number from the directory, not from the counter captured
/// when the writer opened, so it lands after commits made by other writers.
#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn test_commit_relists_to_pick_number(#[case] backend: &str) -> Result<(), BundlebaseError> {
    init();
    let loc = location(backend);
    let path = loc.path.as_str();

    let builder = BundleBuilder::create(path, None).await?;
    builder.set_name("base").await?;
    builder.commit("initial").await?;

    // Both writers open at manifest 1.
    let a = writer(path).await?;
    let b = writer(path).await?;

    let before = manifest_names(path).await?;
    a.set_description("from a").await?;
    a.commit("a1").await?;
    let a1 = new_names(path, &before).await?.remove(0);
    assert_eq!(number(&a1), 2);

    let before = manifest_names(path).await?;
    b.set_description("from b").await?;
    b.commit("b1").await?;
    let b1 = new_names(path, &before).await?.remove(0);
    assert_eq!(
        number(&b1),
        3,
        "B must number after A's commit, not after its own stale counter"
    );

    let before = manifest_names(path).await?;
    a.set_description("from a again").await?;
    a.commit("a2").await?;
    let a2 = new_names(path, &before).await?.remove(0);
    assert_eq!(number(&a2), 4, "A must number after B's commit");
    Ok(())
}

/// Extending to a new directory starts that directory's numbering at 1 and
/// never renumbers or adds to the source bundle's manifests.
#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn test_extend_numbers_each_directory_independently(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    init();
    let src = location(backend);
    let dst = location(backend);

    let builder = BundleBuilder::create(&src.path, None).await?;
    builder.set_name("base").await?;
    builder.commit("one").await?;
    builder.set_description("two").await?;
    builder.commit("two").await?;
    let src_before = manifest_names(&src.path).await?;
    assert_eq!(
        src_before.iter().map(|n| number(n)).collect::<Vec<_>>(),
        vec![1, 2]
    );

    let ext = BundleBuilder::extend(Bundle::open(&src.path, None).await?, Some(&dst.path)).await?;
    ext.set_name("extended").await?;
    ext.commit("extended one").await?;
    ext.set_description("extended two").await?;
    ext.commit("extended two").await?;
    let dst_names = manifest_names(&dst.path).await?;
    assert_eq!(
        dst_names.iter().map(|n| number(n)).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(manifest_names(&src.path).await?, src_before);

    let more = writer(&src.path).await?;
    more.set_description("three").await?;
    more.commit("three").await?;
    let src_after = manifest_names(&src.path).await?;
    assert_eq!(
        src_after.iter().map(|n| number(n)).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(manifest_names(&dst.path).await?, dst_names);

    let reopened = Bundle::open(&dst.path, None).await?;
    assert_eq!(reopened.name().as_deref(), Some("extended"));
    Ok(())
}
