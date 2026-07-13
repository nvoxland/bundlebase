//! Column descriptions (DESCRIBE COLUMN) are durable, manifest-carried
//! metadata: they must survive commit + reopen so the semantic context an
//! agent relies on travels with the bundle.

use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::{BundleBuilderExt, BundleFacadeCommandExt};
use bundlebase_common::BundlebaseError;

mod common;

#[tokio::test]
async fn test_column_description_persists_across_commit_reopen() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    // Set a column description via the SQL command, then commit.
    builder
        .as_ref()
        .execute("DESCRIBE COLUMN salary AS 'annual gross pay in USD'", vec![])
        .await?;

    // Visible in-session (from the operation list).
    let salary_id = builder.column_id("salary").expect("salary column");
    assert_eq!(
        builder.column_descriptions().get(&salary_id).map(|s| s.as_str()),
        Some("annual gross pay in USD"),
        "description must be visible in-session"
    );

    builder.commit("describe salary").await?;

    // Reopen from disk — the description must have been persisted in the manifest.
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let reopened_id = reopened.column_id("salary").expect("salary column after reopen");
    assert_eq!(
        reopened
            .column_descriptions()
            .get(&reopened_id)
            .map(|s| s.as_str()),
        Some("annual gross pay in USD"),
        "column description must survive commit + reopen (manifest-carried)"
    );

    Ok(())
}

/// A later DESCRIBE COLUMN overrides an earlier one for the same column.
#[tokio::test]
async fn test_column_description_last_write_wins() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    builder
        .as_ref()
        .execute("DESCRIBE COLUMN salary AS 'first'", vec![])
        .await?;
    builder
        .as_ref()
        .execute("DESCRIBE COLUMN salary AS 'second'", vec![])
        .await?;

    let id = builder.column_id("salary").expect("salary");
    assert_eq!(
        builder.column_descriptions().get(&id).map(|s| s.as_str()),
        Some("second"),
        "the most recent description must win"
    );
    Ok(())
}

/// Metrics (CREATE METRIC) are durable, manifest-carried semantic definitions:
/// they must survive commit + reopen and resolve last-write-wins by name.
#[tokio::test]
async fn test_metric_persists_across_commit_reopen() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    builder
        .as_ref()
        .execute(
            "CREATE METRIC total_salary AS 'SUM(salary)' DESCRIPTION 'gross payroll'",
            vec![],
        )
        .await?;
    // Redefine to prove last-write-wins.
    builder
        .as_ref()
        .execute("CREATE METRIC total_salary AS 'SUM(salary) / 1000'", vec![])
        .await?;

    builder.commit("define metric").await?;
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let metrics = reopened.metrics();
    let m = metrics
        .iter()
        .find(|m| m.name == "total_salary")
        .expect("metric after reopen");
    assert_eq!(m.expression, "SUM(salary) / 1000", "last definition must win");
    // The redefinition dropped the description (a fresh CREATE METRIC).
    assert_eq!(m.description, None);
    Ok(())
}

/// DROP METRIC removes a metric from resolution and the removal persists.
#[tokio::test]
async fn test_drop_metric_removes_and_persists() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    builder
        .as_ref()
        .execute("CREATE METRIC total_salary AS 'SUM(salary)'", vec![])
        .await?;
    assert_eq!(builder.metrics().len(), 1, "metric defined");

    builder
        .as_ref()
        .execute("DROP METRIC total_salary", vec![])
        .await?;
    assert!(
        builder.metrics().is_empty(),
        "dropped metric must not resolve in-session"
    );

    builder.commit("define then drop metric").await?;
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    assert!(
        reopened.metrics().is_empty(),
        "the drop must persist across commit + reopen"
    );
    Ok(())
}

/// A DROP followed by a later DEFINE of the same name re-adds the metric.
#[tokio::test]
async fn test_drop_then_redefine_metric() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    builder
        .as_ref()
        .execute("CREATE METRIC total_salary AS 'SUM(salary)'", vec![])
        .await?;
    builder
        .as_ref()
        .execute("DROP METRIC total_salary", vec![])
        .await?;
    builder
        .as_ref()
        .execute("CREATE METRIC total_salary AS 'SUM(salary) * 2'", vec![])
        .await?;

    let metrics = builder.metrics();
    assert_eq!(metrics.len(), 1, "the re-definition must resolve exactly once");
    assert_eq!(metrics[0].expression, "SUM(salary) * 2");
    Ok(())
}

/// Column descriptions surface as Arrow field metadata via `described_schema`,
/// so Flight/BI consumers see them, not just MCP agents.
#[tokio::test]
async fn test_described_schema_carries_column_descriptions() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;
    builder
        .as_ref()
        .execute("DESCRIBE COLUMN salary AS 'annual gross pay in USD'", vec![])
        .await?;

    let schema = builder.described_schema().await?;
    let salary = schema
        .fields()
        .iter()
        .find(|f| f.name() == "salary")
        .expect("salary field");
    assert_eq!(
        salary.metadata().get("description").map(|s| s.as_str()),
        Some("annual gross pay in USD"),
        "described_schema must attach the description as field metadata"
    );

    // A column with no description carries no such metadata key.
    let other = schema
        .fields()
        .iter()
        .find(|f| f.name() != "salary")
        .expect("another field");
    assert!(
        !other.metadata().contains_key("description"),
        "undescribed columns must not gain a description key"
    );
    Ok(())
}
