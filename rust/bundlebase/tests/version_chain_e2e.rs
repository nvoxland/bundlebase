//! Manifest version boundaries must agree between in-session commit and replay.
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use rstest::rstest;

mod common;

#[rstest]
#[case("memory")]
#[case("file")]
#[tokio::test]
async fn test_committed_versions_match_reopen_with_edit_ops(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    common::init_catalog();
    let temporary = tempfile::tempdir()?;
    let path = if backend == "memory" {
        random_memory_url().to_string()
    } else {
        temporary.path().to_string_lossy().into_owned()
    };
    let builder = BundleBuilder::create(&path, None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    assert_eq!(builder.version(), "UNCOMMITTED");
    builder.commit("attach").await?;
    assert_eq!(
        builder.version(),
        Bundle::open(&path, None).await?.version()
    );

    builder.delete("id = 1").await?;
    builder.update("SET salary = 123 WHERE id = 2").await?;
    assert_eq!(builder.version(), "UNCOMMITTED");
    builder
        .commit("edit files are part of the manifest hash")
        .await?;
    let source = Bundle::open(&path, None).await?;
    assert_eq!(builder.version(), source.version());
    let previous = source.version();
    let independent = source.extend(None).await?;
    independent.set_name("updated").await?;
    assert_eq!(independent.version(), "UNCOMMITTED");
    assert_eq!(source.version(), previous);
    independent.commit("name").await?;
    assert_eq!(source.version(), previous);
    assert_eq!(
        independent.version(),
        Bundle::open(&path, None).await?.version()
    );

    let child_path = if backend == "memory" {
        random_memory_url().to_string()
    } else {
        temporary
            .path()
            .join("child")
            .to_string_lossy()
            .into_owned()
    };
    let child = independent.extend(Some(&child_path)).await?;
    child.set_name("child").await?;
    child.commit("child inherits manifest chain").await?;
    assert_eq!(
        child.version(),
        Bundle::open(&child_path, None).await?.version()
    );
    Ok(())
}
