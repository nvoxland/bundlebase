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
    // Updating the expression leaves independently owned metadata intact.
    assert_eq!(m.description.as_deref(), Some("gross payroll"));
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

/// Drain a query stream into the first value of the first column as f64.
async fn first_f64(
    mut stream: datafusion::execution::SendableRecordBatchStream,
) -> Result<f64, BundlebaseError> {
    use futures::StreamExt;
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        if batch.num_rows() == 0 {
            continue;
        }
        let col = arrow::compute::cast(batch.column(batch.num_columns() - 1), &arrow::datatypes::DataType::Float64)?;
        let arr = col
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .ok_or("expected a Float64 column")?;
        return Ok(arr.value(0));
    }
    Err("query returned no rows".into())
}

/// Metric expressions reference columns by stable id, so a metric keeps
/// evaluating after RENAME COLUMN, and listings show the *current* name.
#[tokio::test]
async fn test_metric_survives_rename_column() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    builder
        .as_ref()
        .execute("CREATE METRIC payroll AS 'SUM(salary)'", vec![])
        .await?;
    let before = first_f64(builder.compute_metric("payroll", &[], None).await?).await?;

    builder
        .as_ref()
        .execute("RENAME COLUMN salary TO \"Annual Pay\"", vec![])
        .await?;

    let metrics = builder.metrics();
    assert_eq!(
        metrics[0].expression, "SUM(\"Annual Pay\")",
        "listing must show the current user-visible column name"
    );
    let after = first_f64(builder.compute_metric("payroll", &[], None).await?).await?;
    assert_eq!(before, after, "metric must still evaluate after the rename");

    // And it survives commit + reopen.
    builder.commit("rename").await?;
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    assert_eq!(reopened.metrics()[0].expression, "SUM(\"Annual Pay\")");
    let reopened_value =
        first_f64(reopened.compute_metric("payroll", &[], None).await?).await?;
    assert_eq!(before, reopened_value);
    Ok(())
}

/// Metric expressions are planned against the bundle at definition time, so a
/// typo'd column fails immediately instead of when an agent first uses it.
#[tokio::test]
async fn test_define_metric_rejects_unknown_column() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;

    let err = builder
        .as_ref()
        .execute("CREATE METRIC bad AS 'SUM(salaryy)'", vec![])
        .await
        .err()
        .ok_or("unknown column must be rejected")?;
    assert!(err.to_string().contains("salaryy"), "error names the bad column: {err}");

    let err = builder
        .as_ref()
        .execute("CREATE METRIC bad AS 'SUM(salary'", vec![])
        .await
        .err()
        .ok_or("syntax error must be rejected")?;
    assert!(err.to_string().contains("bad"), "error names the metric: {err}");
    assert!(builder.metrics().is_empty(), "a rejected metric is not recorded");
    Ok(())
}

/// Column descriptions are keyed by ColumnId, so they follow the column
/// through RENAME COLUMN.
#[tokio::test]
async fn test_column_description_survives_rename() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;
    builder.describe_column("salary", "annual gross pay").await?;
    builder.rename_column("salary", "pay").await?;

    let by_name = builder.column_descriptions_by_name();
    assert_eq!(by_name.get("pay").map(|s| s.as_str()), Some("annual gross pay"));
    assert!(!by_name.contains_key("salary"));
    Ok(())
}

/// DROP METRIC is reachable from the Rust builder API, not just SQL.
#[tokio::test]
async fn test_drop_metric_builder_method() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;
    builder
        .define_metric("headcount", "COUNT(*)", None)
        .await?
        .drop_metric("headcount")
        .await?;
    assert!(builder.metrics().is_empty());
    Ok(())
}

/// compute_metric groups by dimensions and validates them.
#[tokio::test]
async fn test_compute_metric_grouped_and_errors() -> Result<(), BundlebaseError> {
    use futures::StreamExt;
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder.attach(test_datafile("userdata.parquet"), None).await?;
    builder.define_metric("headcount", "COUNT(*)", None).await?;

    let mut stream = builder
        .compute_metric("headcount", &["gender".to_string()], Some(50))
        .await?;
    let mut rows = 0;
    while let Some(b) = stream.next().await {
        let b = b?;
        assert!(b.schema().field_with_name("gender").is_ok());
        assert!(b.schema().field_with_name("headcount").is_ok());
        rows += b.num_rows();
    }
    assert!(rows >= 2, "grouping by gender yields several groups");

    let err = builder
        .compute_metric("nope", &[], None)
        .await
        .err()
        .ok_or("unknown metric must error")?;
    assert!(err.to_string().contains("headcount"), "lists available: {err}");

    let err = builder
        .compute_metric("headcount", &["not_a_col".to_string()], None)
        .await
        .err()
        .ok_or("unknown dimension must error")?;
    assert!(err.to_string().contains("not_a_col"), "{err}");
    Ok(())
}

/// data_quality only skips the empty-bundle sentinel, never a real column
/// that happens to be named `no_data`.
#[tokio::test]
async fn test_data_quality_keeps_real_no_data_column() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    let empty = builder.data_quality().await?;
    assert_eq!(empty.total_rows, 0);
    assert!(empty.columns.is_empty(), "the empty-bundle sentinel is skipped");

    builder.attach(test_datafile("userdata.parquet"), None).await?;
    builder.rename_column("gender", "no_data").await?;
    let report = builder.data_quality().await?;
    assert!(report.total_rows > 0);
    let col = report
        .columns
        .iter()
        .find(|c| c.name == "no_data")
        .ok_or("a real column named no_data must be reported")?;
    assert!(col.distinct >= 2);
    Ok(())
}
