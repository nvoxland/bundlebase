//! IMPORT JOIN publishes one durable manifest per nonempty source commit.
use bundlebase::bundle::{BundleFacade, JoinTypeOption};
use bundlebase::test_utils::{random_memory_url, test_datafile};
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_command::parser::parse_command;
use bundlebase_command::{BundleBuilderExt, BundleFacadeCommandExt};
use bundlebase_common::BundlebaseError;

mod common;

#[tokio::test]
async fn checkpointed_import_join_commits_active_changes_once() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let source_url = random_memory_url();
    let source = BundleBuilder::create(source_url.as_str(), None).await?;
    source
        .attach(test_datafile("sales-regions.csv"), None)
        .await?;
    source.commit("original regions").await?;
    source
        .attach(test_datafile("sales-regions.csv"), None)
        .await?;
    source.commit("second regions batch").await?;
    source.checkpoint().await?;

    let target_url = random_memory_url();
    let target = BundleBuilder::create(target_url.as_str(), None).await?;
    target
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    let source_location = format!("bundle+{source_url}");
    target
        .join(
            "regions",
            r#"bundle."Country" = regions."Country""#,
            Some(&source_location),
            JoinTypeOption::Inner,
        )
        .await?;
    target.commit("target with external join").await?;

    target
        .execute_command(parse_command("IMPORT JOIN regions")?)
        .await?;
    assert!(
        target.status().is_empty(),
        "published active changes must not be recorded again"
    );
    let history = target.history();
    let imported = history
        .iter()
        .filter(|commit| commit.message.starts_with("[import regions]"))
        .collect::<Vec<_>>();
    assert_eq!(imported.len(), 2);
    assert_eq!(imported[0].message, "[import regions] original regions");
    assert_eq!(imported[1].message, "[import regions] second regions batch");
    assert!(imported
        .iter()
        .all(|commit| !commit.operations().is_empty()));
    assert_eq!(history.len(), 3, "no empty outer-scope manifest");

    let reopened = Bundle::open(target_url.as_str(), None).await?;
    assert_eq!(target.version(), reopened.version());
    assert_eq!(target.operations(), reopened.operations());
    assert_eq!(
        target
            .history()
            .iter()
            .map(|commit| &commit.message)
            .collect::<Vec<_>>(),
        reopened
            .history()
            .iter()
            .map(|commit| &commit.message)
            .collect::<Vec<_>>()
    );
    Ok(())
}
